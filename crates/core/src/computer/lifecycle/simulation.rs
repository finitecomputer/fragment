//! The lifecycle, its saves and its images under any interleaving of the
//! DO's reports: the invariants each step keeps.

use fragment_proto::computer::{ComputerNotice, LifeEnd};

use super::*;
use crate::computer::testing::{rec, IMAGE, T};
use crate::computer::*;

/// Goal: under any interleaving of events (duplicates, late reports,
/// reordering, crashes, snapshots that will not start, saves that will
/// not restore, holds the guest answers or not, saves that fail), the
/// generation only grows, at most one container is ever running,
/// metered intervals never overlap or run backwards, a computer is
/// never left asleep while it is wanted and not failed, one that dies
/// busy starts again unless it gave up, and each life (a start that
/// came up) ends once. A computer is never asleep with work newer than
/// its newest save unless a crash or the bounded failure put it there,
/// and awake and idle with work newer than its newest save, a save is
/// under way or due . An exit nothing asked for while it is idle, all its
/// life did in a save of its own, is a sleep: no strike, no rollback, and
/// no start unless always on. Its saves ride along as the DO keeps them, so
/// their own assertions run too. Method: the DO's reports for the
/// actions it was handed are delivered in any order, late, twice, or
/// never, among events of any kind.
#[test]
fn simulated_interleavings_keep_the_invariants() {
    // a small xorshift: the sequence is a pure function of the seed
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }
    // the sim's bound is shorter than the default, so it is reached
    let rules = Rules { unsaved_max_ms: 10 * 60_000 };
    let mut reached = [0u32; 14];
    // the images deploys pin, one after another and back
    let deployed = [IMAGE, "registry/stub@sha256:2", "registry/stub@sha256:3"];
    for seed0 in 1..=8u64 {
        let mut seed = seed0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut l = Lifecycle::new();
        let mut saves = Saves::default();
        let mut images = Images::default();
        images.looked(Some(IMAGE));
        let mut now = T;
        let mut metered: Vec<(i64, i64)> = vec![];
        let mut generation = 0;
        let mut came_up = std::collections::BTreeSet::new();
        let mut ended = std::collections::BTreeSet::new();
        // what the DO would report next, for the actions it was handed
        let mut pending: Vec<Event> = vec![];
        // each save asked for, by its number: its start, when, and held
        let mut taken = std::collections::BTreeMap::new();
        let mut newest_save_ms = i64::MIN;
        let mut last_work_ms = i64::MIN;
        // what was done on the running life (the lifecycle's `worked`)
        // and the time its newest save kept was taken (its start before
        // any), and the save that holds what it did up to then: its
        // own newest kept, else the one it came up from; and of the
        // last life that ended, whether nothing was done since then
        let (mut life_worked, mut life_kept, mut life_save) = (i64::MIN, i64::MIN, None::<String>);
        let mut lost: Option<(u64, bool, Option<String>)> = None;
        // half the seeds' saves fail often enough to reach the bound
        let fails_one_in = if seed0 % 2 == 0 { 2 } else { 5 };
        for step in 0..6_000u32 {
            now += (rng(&mut seed) % 120_000) as i64;
            let g = l.generation().saturating_sub(rng(&mut seed) % 2);
            let event = if !pending.is_empty() && !rng(&mut seed).is_multiple_of(3) {
                let i = (rng(&mut seed) % pending.len() as u64) as usize;
                pending.remove(i)
            } else {
                let seq = l.save_seq.saturating_sub(rng(&mut seed) % 2);
                match rng(&mut seed) % 17 {
                    // its owner's restart, of the newest start or an earlier one
                    14 if rng(&mut seed).is_multiple_of(2) => {
                        if l.restarts(g) && !matches!(l.phase, Phase::Starting { .. }) {
                            // the DO's: the restart's start is fresh
                            saves.restart_fresh();
                        }
                        Event::Restart { generation: g }
                    }
                    0 => Event::Wake { why: [Wake::Record, Wake::Presence, Wake::Tab, Wake::Owner, Wake::Joined][(rng(&mut seed) % 5) as usize] },
                    // a record for its agents, posted a while before its wake comes
                    15 => Event::Posted { at_ms: now - (rng(&mut seed) % 5_000) as i64 },
                    1 => Event::Ready { generation: g },
                    2 => Event::StartFailed { generation: g, why: "sim".into() },
                    3 => Event::Opened { socket: if rng(&mut seed).is_multiple_of(2) { Socket::Tab } else { Socket::Keepalive } },
                    4 => Event::Closed { socket: if rng(&mut seed).is_multiple_of(2) { Socket::Tab } else { Socket::Keepalive } },
                    5 => Event::Asleep { generation: g },
                    6 => Event::Exited { generation: g },
                    7 => Event::AlwaysOn { on: rng(&mut seed).is_multiple_of(4) },
                    8 if rng(&mut seed).is_multiple_of(3) => Event::Sleep,
                    9 => Event::SnapshotFailed { generation: g, why: "sim".into() },
                    10 => {
                        // the DO found the save a start restores unusable
                        let starting = matches!(l.phase, Phase::Starting { generation: s, .. } if s == g);
                        let current = saves.current().map(|c| c.id.clone()).filter(|_| starting);
                        let older = current.is_some_and(|id| saves.unusable(&id));
                        Event::RestoreFailed { generation: g, why: "sim".into(), older }
                    }
                    // late or repeated answers of the DO's
                    11 => Event::Held { generation: g, held: rng(&mut seed).is_multiple_of(2) },
                    12 => Event::Saved { generation: g, seq },
                    13 => Event::SaveFailed { generation: g, seq, why: "sim".into() },
                    _ => Event::Alarm,
                }
            };
            // work: a guest holding its keepalive while awake
            if matches!(l.phase, Phase::Awake { .. }) && l.keepalives > 0 {
                last_work_ms = now;
            }
            // a crash of the running start while its guest holds a keepalive
            let busy_crash = matches!(&event, Event::Exited { generation: e } if matches!(l.phase, Phase::Awake { generation, .. } if generation == *e)) && l.keepalives > 0;
            // a start that failed, however it was heard (its report, or its container's end)
            // (its own destroy's exit is no failure of its own: its report is)
            let own_destroy = matches!(&event, Event::Exited { generation: e } if matches!(l.phase, Phase::Starting { generation, destroying: true, .. } if generation == *e));
            let start_failed = !own_destroy && matches!(&event, Event::Exited { generation: e } | Event::StartFailed { generation: e, .. } if matches!(l.phase, Phase::Starting { generation, .. } if generation == *e));
            let failures_before = l.failures;
            // an exit nothing asked for, of an idle life whose work is kept
            let unasked_sleep = matches!(&event, Event::Exited { generation: e } if matches!(l.phase, Phase::Awake { generation, .. } if generation == *e)) && l.idle_and_kept();
            let coming_up = matches!(&event, Event::Ready { generation: e } if matches!(l.phase, Phase::Starting { generation, .. } if generation == *e));
            let kept = match &event {
                Event::Saved { generation: e, seq } if l.running() == Some(*e) => matches!(l.saving, Some(Saving::Save { seq: s, .. }) if s == *seq).then_some(*seq),
                _ => None,
            };
            let work = l.ready
                && match &event {
                    Event::Opened { .. } => true,
                    Event::Closed { socket: Socket::Keepalive } => l.keepalives > 0,
                    Event::Closed { socket: Socket::Tab } => l.tabs > 0,
                    Event::Wake { why } => matches!(why, Wake::Record | Wake::Joined),
                    _ => false,
                };
            let cancels = matches!(&event, Event::Opened { socket: Socket::Keepalive }) && matches!(l.phase, Phase::Sleeping { .. }) && matches!(l.phase, Phase::Sleeping { owner: false, .. }) && matches!(l.saving, Some(Saving::Hold { .. }));
            let free_before = l.unmetered;
            reached[9] += u32::from(free_before.is_some());
            let s = l.apply_with(event, now, &rules);
            // stored and read again (its isolate gone): the same computer
            let stored = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap();
            assert_eq!(stored, l, "a stored lifecycle reads back as it was (seed {seed0}, step {step})");
            l = stored;
            if work {
                life_worked = now;
            }
            if let Some(seq) = kept {
                // the save the DO keeps, as it reports it kept
                let (of, at, held) = taken[&seq];
                saves.saved(of, rec(&format!("b{seed0}-{seq}")), now, held);
                newest_save_ms = at;
                reached[0] += 1;
                (life_kept, life_save) = (at, Some(format!("b{seed0}-{seq}")));
            }
            reached[3] += u32::from(cancels);
            if let Some(e) = s.ended {
                assert!(came_up.contains(&e.generation), "only a start that came up ends a life (seed {seed0})");
                assert!(ended.insert(e.generation), "a life ends once (seed {seed0})");
                if e.by == LifeEnd::Sleep && e.saved {
                    assert!(last_work_ms <= newest_save_ms, "asleep with work newer than its newest save (seed {seed0}, step {step}): {last_work_ms} > {newest_save_ms}");
                }
                saves.ended(e);
                lost = Some((e.generation, life_worked < life_kept, life_save.clone()));
            }
            if coming_up {
                assert!(matches!(l.phase, Phase::Awake { .. }));
                came_up.insert(l.generation());
                // the DO's plan looks, and starts what it saw
                let plan = saves.plan(images.next());
                let from = plan.source();
                let save = saves.current().map(|c| c.id.clone()).filter(|_| plan != Plan::Nothing);
                saves.starting(l.generation(), from);
                let r = saves.came_up(l.generation(), from, save.as_deref(), images.next(), now).expect("the start that came up records what it restored");
                // a rollback its owner is not told of lost nothing: nothing
                // was done on the life it lost since the save it went back to
                if r.rollback && !saves.went_back().is_some_and(|w| w.restored.generation == r.generation) {
                    let (life, quiet, kept_in) = lost.clone().expect("a rollback follows a life's end");
                    assert!(quiet && kept_in == r.save, "a rollback of life {life} told nothing though it lost work (seed {seed0}, step {step}): {r:?}");
                    reached[11] += 1;
                }
                lost = None;
                (life_worked, life_kept, life_save) = (if l.open() { now } else { i64::MIN }, now, r.save.clone());
                // a new life's /data is its save's: no work of its own yet
                last_work_ms = i64::MIN;
            }
            if busy_crash && !matches!(l.phase, Phase::Failed { .. }) {
                // made by the alarm it asks for at once
                assert!(l.start_due(now) && s.alarm_ms == Some(now), "busy when it died, it starts again (seed {seed0}): {l:?}");
            }
            if unasked_sleep {
                assert_eq!(s.ended.map(|e| (e.by, e.saved, e.quiet)), Some((LifeEnd::Sleep, true, true)), "an idle, kept life's exit is a sleep (seed {seed0}): {s:?}");
                assert_eq!(l.failures, failures_before, "no strike (seed {seed0})");
                assert_eq!(l.start_due(now), l.always_on, "no start but always on's (seed {seed0}): {l:?}");
                reached[13] += 1;
            }
            if start_failed {
                assert_eq!(l.failures, failures_before + 1, "a failed start is one strike (seed {seed0}): {l:?}");
                assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "a failed start waits its pause (seed {seed0}): {s:?}");
                assert!(matches!(l.phase, Phase::Failed { .. }) || matches!(l.phase, Phase::Asleep { retry: Some(_) }), "{l:?}");
            }
            if matches!(s.note, SaveNote::Says(ref why) if why.contains("slept unsaved")) {
                reached[2] += 1;
            }
            for a in &s.actions {
                match a {
                    Action::Start { generation: g } => {
                        assert!(*g > generation, "a start's generation grows");
                        generation = *g;
                    }
                    Action::Meter { from_ms, to_ms } => {
                        assert!(from_ms < to_ms && *to_ms <= now, "an interval runs forward, up to now");
                        assert!(metered.last().is_none_or(|(_, last)| from_ms >= last), "intervals never overlap");
                        // the time kept for failed saves is free: no interval
                        // overlaps the window open before or after this step
                        for (a, b) in [free_before, l.unmetered].into_iter().flatten() {
                            assert!(*to_ms <= a || *from_ms >= b.min(now), "metered {from_ms}..{to_ms} in the free window {a}..{b} (seed {seed0})");
                        }
                        metered.push((*from_ms, *to_ms));
                    }
                    Action::Sleep { generation: g } | Action::Hold { generation: g } => {
                        assert_eq!(*g, l.generation(), "only the current start holds");
                        // most guests answer the hold
                        pending.push(Event::Held { generation: *g, held: !rng(&mut seed).is_multiple_of(4) });
                        // and some take work as a sleep holds them (the hold race)
                        if matches!(a, Action::Sleep { .. }) && rng(&mut seed).is_multiple_of(3) {
                            pending.push(Event::Opened { socket: Socket::Keepalive });
                        }
                    }
                    Action::Save { generation: g, held, seq } => {
                        assert!(taken.insert(*seq, (*g, now, *held)).is_none(), "a save's number is asked for once");
                        if rng(&mut seed).is_multiple_of(fails_one_in) {
                            pending.push(Event::SaveFailed { generation: *g, seq: *seq, why: "sim".into() });
                            reached[1] += 1;
                        } else {
                            pending.push(Event::Saved { generation: *g, seq: *seq });
                        }
                    }
                    Action::Unhold { generation: g } => assert_eq!(*g, l.generation(), "only the current start is let go"),
                    Action::Stop { generation: g, .. } => {
                        assert_eq!(*g, l.generation(), "only the current start stops");
                        // the stop reports its end; its container's exit,
                        // a report of its own, may come before it or after
                        pending.push(Event::Asleep { generation: *g });
                        if rng(&mut seed).is_multiple_of(2) {
                            pending.push(Event::Exited { generation: *g });
                        }
                    }
                }
            }
            assert!(pending.len() < 64, "the DO's reports are bounded by the actions it is handed");
            assert!(saves.rollbacks() <= ended.len() as u64, "each rollback is one life's loss");
            if matches!(l.phase, Phase::Asleep { retry: None }) && l.wanted(now) {
                panic!("asleep while wanted, with no retry due (seed {seed0}): {l:?}");
            }
            // I7: awake and idle, work newer than its newest save is saved soon
            if matches!(l.phase, Phase::Awake { .. }) && l.keepalives == 0 && last_work_ms > newest_save_ms {
                assert!(l.saving.is_some() || l.save_due_ms.is_some(), "awake with work unsaved and no save due (seed {seed0}, step {step}): {l:?}");
                reached[4] += 1;
            }
            if l.unsaved_since_ms.is_some() && matches!(l.phase, Phase::Awake { .. }) {
                reached[5] += 1;
            }
            // whatever is running has an alarm watching it (a past one fires at once)
            if !matches!(l.phase, Phase::Asleep { .. } | Phase::Failed { .. }) {
                assert!(s.alarm_ms.is_some(), "a running computer has its alarm: {l:?}");
            }
            if s.ended.is_some_and(|e| e.restart) {
                reached[6] += 1;
            }
            // a deploy now and then: the DO's next look finds another image
            if rng(&mut seed).is_multiple_of(29) {
                images.looked(Some(deployed[(rng(&mut seed) % 3) as usize]));
            }
            // and, now and then, some of the keepalives it counts are
            // disconnected without a close it hears (#557): the DO closes
            // those its runtime no longer holds, never one it holds
            if l.keepalives() > 0 && rng(&mut seed).is_multiple_of(7) {
                let held = (rng(&mut seed) % u64::from(l.keepalives() + 1)) as usize;
                let (before, lost) = (l.keepalives(), l.keepalives_lost(held));
                for _ in 0..lost {
                    let s = l.apply_with(Event::Closed { socket: Socket::Keepalive }, now, &rules);
                    assert!(s.actions.is_empty() && s.ended.is_none(), "a close acts on nothing at once (seed {seed0}): {s:?}");
                }
                assert!(lost >= 1 || before as usize <= held, "one counted beyond what is held is lost (seed {seed0})");
                assert!(l.keepalives() == before - lost && l.keepalives() as usize >= held, "closed down to what the runtime holds, never below (seed {seed0}): {held} held, {before} counted");
                reached[12] += u32::from(lost > 0);
            }
            // what its owner is told is its state's, whatever happened
            let told = notices(&l, &saves, &images, &rules);
            for n in &told {
                match n {
                    ComputerNotice::WontWake { .. } => assert!(matches!(l.phase, Phase::Failed { .. }), "won't wake is told only of one that won't (seed {seed0})"),
                    ComputerNotice::Unsaved { since, failing_since, failures, stops_at, .. } => {
                        assert!(l.running().is_some() && *failures >= 1, "a running computer's saves fail (seed {seed0}): {n:?}");
                        assert!(since <= failing_since, "its work is unsaved since before its first failure (seed {seed0}): {n:?}");
                        if let (Some(at), Some(sleep_failed)) = (stops_at, l.unsaved_since_ms()) {
                            assert!(*at >= sleep_failed + rules.unsaved_max_ms, "never stopped before its bound (seed {seed0}): {n:?}");
                        }
                        reached[7] += 1;
                    }
                    ComputerNotice::WentBack { life, pending, at, .. } => {
                        assert!(*life > saves.seen && *life <= l.generation(), "a loss told is unseen, and of a life it had (seed {seed0}): {n:?}");
                        assert_eq!(*pending, at.is_none(), "a pending loss has no start yet (seed {seed0})");
                        reached[8] += 1;
                    }
                    ComputerNotice::Update { version } => {
                        assert!(matches!(l.phase, Phase::Awake { .. }) && l.keepalives == 0, "an update is told awake, between turns (seed {seed0}): {l:?}");
                        assert_eq!(Some(version), images.latest().as_ref(), "the version its next start runs (seed {seed0})");
                        assert!(version_running(&l, &saves).is_some_and(|v| v != *version), "never the version it runs (seed {seed0})");
                        assert_ne!(images.later.as_ref(), Some(version), "never one put off (seed {seed0})");
                        reached[10] += 1;
                    }
                    ComputerNotice::DiskFilling { total, free } => assert!(!disk_clear(*total, *free), "a disk told filling has no clear room (seed {seed0}): {n:?}"),
                    ComputerNotice::KeyPaused { .. } => panic!("a lifecycle tells no key's pause (seed {seed0})"),
                }
            }
            // its owner puts an update off, now and then
            if let Some(ComputerNotice::Update { version }) = told.iter().find(|n| matches!(n, ComputerNotice::Update { .. })) {
                if rng(&mut seed).is_multiple_of(4) {
                    assert!(images.later(version), "an update told can be put off (seed {seed0})");
                    assert!(!notices(&l, &saves, &images, &rules).iter().any(|n| matches!(n, ComputerNotice::Update { .. })), "put off, it is told no more (seed {seed0})");
                }
            }
            // its owner says they saw one, now and then
            if let Some(ComputerNotice::WentBack { life, .. }) = told.last() {
                if rng(&mut seed).is_multiple_of(5) {
                    assert!(saves.saw(*life), "a loss told can be seen (seed {seed0})");
                    assert!(!notices(&l, &saves, &images, &rules).iter().any(|n| matches!(n, ComputerNotice::WentBack { .. })), "seen, it is told no more (seed {seed0})");
                }
            }
        }
        assert!(!came_up.is_empty() && !ended.is_empty() && saves.rollbacks() > 0, "the simulation reached lives, their ends, and rollbacks (seed {seed0})");
    }
    // every kind of save the lifecycle has was reached, across the seeds
    assert!(reached.iter().all(|n| *n > 0), "saves kept, saves failed, sleeps unsaved, sleeps cancelled, unsaved work due, failed sleeps kept awake, restarts, unsaved told, went back told, free windows, updates told, rollbacks that lost nothing told nothing, lost keepalives closed, idle exits slept: {reached:?}");
}
