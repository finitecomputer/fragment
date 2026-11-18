//! A quiet wake after an image changes (P7): Cloudflare ties a snapshot to
//! the image it was taken on, so after a deploy that changed the image a
//! computer's next wake pulls it and restores its whole save. A computer
//! used lately wakes once on the new image instead, with nobody waiting,
//! and its sleep takes a snapshot there: its person's next wake is a
//! snapshot's. Each Computer object decides it for itself, asleep, at a
//! look its own alarm makes every few hours (`look_at`).

use crate::computer::lifecycle::{Lifecycle, Phase};
use crate::computer::limits::{REFRESH_LOOK_MS, REFRESH_USED_MS};
use crate::computer::saves::Saves;

/// Whether a computer wakes quietly now: it is asleep with no start due,
/// something other than a quiet wake woke it within `REFRESH_USED_MS`
/// (`used_ms`), and its next start runs an image (`next`, its reference)
/// its last start did not come up on. That start would pull the image and
/// restore its save whole, since a snapshot is only ever of the image its
/// life ran (`Saves::plan`). Once it came up on `next`, it is not again.
pub fn refresh_due(life: &Lifecycle, saves: &Saves, next: Option<&str>, used_ms: Option<i64>, now_ms: i64) -> bool {
    let asleep = matches!(life.phase, Phase::Asleep { retry: None });
    let moved = saves.ran().is_some_and(|ran| next.is_some_and(|next| next != ran));
    asleep && used(used_ms, now_ms) && moved
}

/// When an asleep computer looks again (`refresh_due`): between half of
/// `REFRESH_LOOK_MS` and all of it from now (`jitter` picks where), so the
/// fleet's wakes after one deploy spread over hours. None while it is not
/// asleep (its lifecycle keeps its alarm), won't wake, or was not used
/// within `REFRESH_USED_MS`: then nothing looks until something wakes it.
pub fn look_at(life: &Lifecycle, used_ms: Option<i64>, now_ms: i64, jitter: u32) -> Option<i64> {
    let half = REFRESH_LOOK_MS / 2;
    let asleep = matches!(life.phase, Phase::Asleep { retry: None });
    (asleep && used(used_ms, now_ms)).then(|| now_ms + half + i64::from(jitter) % half)
}

fn used(used_ms: Option<i64>, now_ms: i64) -> bool {
    used_ms.is_some_and(|u| now_ms - u < REFRESH_USED_MS)
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::RestoreSource;

    use super::*;
    use crate::computer::testing::{rec, save_asked, Computer, IMAGE, T};
    use crate::computer::*;

    const NEXT: &str = "registry/stub@sha256:2";
    const DAY: i64 = 24 * 60 * 60_000;

    /// A computer used and asleep on `IMAGE`, its sleep's save `b1`
    /// snapshotted there as the DO keeps it.
    fn slept_on_image() -> Computer {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let ran = c.saves.ran().expect("it came up").to_string();
        assert!(c.saves.snapshotted("b1", "s1", &ran));
        c
    }

    /// The quiet wake as the DO makes it: a presence's wake, its start up
    /// on the image it looked at last, held a minute from then, its sleep
    /// saved as `save` and snapshotted on the image it ran.
    fn quiet_life(c: &mut Computer, save: &str) {
        let s = c.apply(Event::Wake { why: Wake::Presence });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("a quiet wake starts it: {s:?}") };
        let restored = c.come_up(generation);
        assert_eq!(restored.from, RestoreSource::Backup, "its start is a full one: the reason it wakes");
        c.apply(Event::Wake { why: Wake::Presence });
        c.now += PREWAKE_MS;
        let s = c.apply(Event::Alarm);
        assert!(s.actions.contains(&Action::Sleep { generation }), "it idles out: {s:?}");
        let (_, held, seq) = save_asked(&c.apply(Event::Held { generation, held: true }));
        c.saves.saved(generation, rec(save), c.now, held);
        let s = c.apply(Event::Saved { generation, seq });
        assert!(s.actions.contains(&Action::Stop { generation, saved: true }), "{s:?}");
        let ran = c.saves.image_of(generation).expect("it came up").to_string();
        assert!(c.saves.snapshotted(save, "s2", &ran));
        c.apply(Event::Asleep { generation });
    }

    /// Goal (valid): asleep, used yesterday, and deployed onto another
    /// image, it wakes quietly once; that life's sleep leaves a snapshot on
    /// the new image, so the next wake is a snapshot's.
    #[test]
    fn a_used_computer_wakes_once_on_a_new_image_and_its_next_wake_is_a_snapshot() {
        let mut c = slept_on_image();
        let used = c.now;
        c.now += DAY;
        assert!(!refresh_due(&c.life, &c.saves, c.images.next(), Some(used), c.now), "its snapshot is on the image it runs next");
        assert!(c.images.looked(Some(NEXT)), "a deploy changed the image");
        assert_eq!(c.saves.plan(Some(NEXT)), Plan::Backup, "its next wake would pull the image and restore its save whole");
        assert!(refresh_due(&c.life, &c.saves, c.images.next(), Some(used), c.now));
        quiet_life(&mut c, "b2");
        assert_eq!(c.saves.plan(Some(NEXT)), Plan::Snapshot { id: "s2".into() }, "the person's next wake is a snapshot's");
        assert_eq!(c.saves.ran(), Some(NEXT));
    }

    /// Goal (invalid): nothing else wakes it. Not used within a week (or
    /// never woken since this was kept), not asleep, a start due, won't
    /// wake, no image to run, or its snapshot already on the new image.
    #[test]
    fn only_an_asleep_used_computer_on_an_old_image_wakes() {
        let mut c = slept_on_image();
        let used = c.now;
        assert!(c.images.looked(Some(NEXT)));
        let due = |c: &Computer, used: Option<i64>| refresh_due(&c.life, &c.saves, c.images.next(), used, c.now);
        assert!(due(&c, Some(used)));
        assert!(!due(&c, None), "never woken since a release kept when");
        let now = c.now;
        c.now = used + REFRESH_USED_MS;
        assert!(!due(&c, Some(used)), "a week since it was used");
        c.now = now;
        assert!(!refresh_due(&c.life, &c.saves, None, Some(used), c.now), "no image by its name");
        let mut never = Computer::new();
        assert!(!refresh_due(&never.life, &never.saves, never.images.next(), Some(never.now), never.now), "a computer never started has nothing to restore");
        never.wake();
        assert!(!due(&never, Some(never.now)), "awake: its own sleep takes the snapshot");
        // a failed start's retry is due: the lifecycle's own alarm makes it
        let mut failing = slept_on_image();
        let s = failing.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        failing.apply(Event::StartFailed { generation, why: "no".into() });
        assert!(matches!(failing.life.phase, Phase::Asleep { retry: Some(_) }));
        assert!(failing.images.looked(Some(NEXT)));
        assert!(!due(&failing, Some(failing.now)), "a start is due");
        for _ in 1..STARTS_FAILED_MAX {
            failing.now += RETRY_PAUSE_MS << STARTS_FAILED_MAX;
            let s = failing.apply(Event::Alarm);
            let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
            failing.apply(Event::StartFailed { generation, why: "no".into() });
        }
        assert!(matches!(failing.life.phase, Phase::Failed { .. }));
        assert!(!due(&failing, Some(failing.now)), "won't wake: only its owner wakes it");
        assert_eq!(look_at(&failing.life, Some(failing.now), failing.now, 0), None, "and nothing looks");
    }

    /// Goal (replay): a look again after the quiet wake, and after one
    /// whose snapshot was lost (or never taken: snapshots off, as on a
    /// local fleet), wakes nothing: once it came up on the new image it
    /// waits for the next.
    #[test]
    fn it_wakes_once_an_image_however_often_it_looks() {
        let mut c = slept_on_image();
        let used = c.now;
        assert!(c.images.looked(Some(NEXT)));
        quiet_life(&mut c, "b2");
        assert!(!refresh_due(&c.life, &c.saves, c.images.next(), Some(used), c.now), "its snapshot is on the new image");
        assert_eq!(c.saves.snapshot_failed().map(|s| s.image), Some(NEXT.to_string()));
        assert!(!refresh_due(&c.life, &c.saves, c.images.next(), Some(used), c.now), "a snapshot lost on the image it ran: no second quiet wake");
        assert!(c.images.looked(Some(IMAGE)), "pinned back");
        assert!(refresh_due(&c.life, &c.saves, c.images.next(), Some(used), c.now), "another image, another quiet wake");
    }

    /// Goal (restart): the decision is a function of what the DO stores,
    /// so an object that comes up again (a deploy, an eviction) decides
    /// the same from its stored lifecycle and saves.
    #[test]
    fn a_stored_computer_decides_the_same() {
        let mut c = slept_on_image();
        let used = c.now;
        assert!(c.images.looked(Some(NEXT)));
        let life = Lifecycle::read(&serde_json::to_string(&c.life).unwrap()).unwrap().0;
        let saves: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert!(refresh_due(&life, &saves, Some(NEXT), Some(used), c.now));
        quiet_life(&mut c, "b2");
        let saves: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert!(!refresh_due(&c.life, &saves, Some(NEXT), Some(used), c.now), "its quiet life is kept");
        assert_eq!(saves.plan(Some(NEXT)), Plan::Snapshot { id: "s2".into() });
    }

    /// Goal: an asleep computer used within the week looks again within
    /// `REFRESH_LOOK_MS`, never sooner than half of it (a fleet's looks
    /// spread over hours); awake, or unused for a week, nothing looks.
    #[test]
    fn it_looks_again_within_hours_while_used_lately() {
        let c = slept_on_image();
        for jitter in [0, 1, u32::MAX, 12_345_678] {
            let at = look_at(&c.life, Some(c.now), c.now, jitter).expect("used today");
            assert!((c.now + REFRESH_LOOK_MS / 2..c.now + REFRESH_LOOK_MS).contains(&at), "{jitter}: {at}");
        }
        assert_eq!(look_at(&c.life, Some(T), T + REFRESH_USED_MS, 0), None, "unused for a week");
        assert_eq!(look_at(&c.life, None, c.now, 0), None);
        let mut awake = Computer::new();
        awake.wake();
        assert_eq!(look_at(&awake.life, Some(awake.now), awake.now, 0), None, "its lifecycle keeps its alarm");
    }
}
