//! An object's one alarm (a Durable Object has one): when to set it.

/// The time to set the alarm to, given the time `set` it is set to now, if
/// any, and the earliest `at` its due work asks for, at `now`, no sooner
/// than `min_ms` from it: `None` when work is due at once (`at` is that
/// soonest) and the alarm already set is still to come and no later, so it
/// stays. Arms one after another (a burst of calls, each with work due at
/// once) then never push a coming alarm out, each a moment past the last,
/// past the burst's end. Work due later is set as asked: a sooner alarm,
/// its work gone (the meter's minute closed early), moves out to it.
pub fn to_set(set: Option<i64>, at: i64, now: i64, min_ms: i64) -> Option<i64> {
    match set {
        Some(set) if set > now && set <= at && at <= now + min_ms => None,
        _ => Some(at),
    }
}

/// Whether the alarm's pass leaves a due card's shot (a browser's seconds,
/// which an object's next alarm waits for) to its next pass: while runs
/// wait to start, they go first, but a card due `max_ms` or longer is shot
/// all the same, so a busy fragment's runs never keep it off for good.
pub fn card_waits(runs_waiting: bool, card_due_at: Option<i64>, now: i64, max_ms: i64) -> bool {
    runs_waiting && card_due_at.is_some_and(|at| now - at < max_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a run queued while the pass works starts before its card's
    /// shot, yet a busy fragment's runs never keep its card off for good
    /// (the review of #1467: a run queued every 100 ms, each pass's batch
    /// in flight when the next came). Valid: runs waiting and a card just
    /// due, the card waits; due `max_ms` or longer, it is shot. Invalid
    /// cases wait for nothing: no run waiting, or no card due.
    #[test]
    fn a_card_waits_for_queued_runs_but_not_for_good() {
        let (now, max) = (100_000, 10_000);
        assert!(card_waits(true, Some(now - 50), now, max), "a card just due waits for the runs");
        assert!(!card_waits(true, Some(now - max), now, max), "one due this long is shot all the same");
        assert!(!card_waits(false, Some(now - 50), now, max), "no run waiting: shot now");
        assert!(!card_waits(true, None, now, max), "no card due: nothing waits");
        // a pass every 300 ms, runs waiting at each: the card is shot at the
        // first pass once it has been due `max_ms`
        let (due, every) = (now, 300);
        let shot = (0..1_000).map(|k| due + every * k).find(|&t| !card_waits(true, Some(due), t, max));
        assert!(shot.is_some_and(|t| (max..max + every).contains(&(t - due))), "shot by {shot:?}");
    }

    /// Goal: a burst of calls never starves what is due (the review of
    /// #1460: arms every 40 ms each moved a 50 ms alarm out by 40 ms, so
    /// the first queued run waited for the burst's end). Valid: a coming
    /// alarm no later than asked stays; none set, one passed, or one later
    /// than asked is set to what is asked.
    #[test]
    fn a_coming_alarm_is_never_pushed_out() {
        let now = 1_000;
        let min = 50;
        assert_eq!(to_set(Some(now + 30), now + min, now, min), None, "the next arm of a burst keeps it");
        assert_eq!(to_set(Some(now + 50), now + min, now, min), None);
        // a burst: an arm every 40 ms, each asking 50 ms out: an alarm still
        // to come never moves later
        let mut set: Option<i64> = None;
        for k in 0..10 {
            let t = now + 40 * k;
            let before = set;
            if let Some(at) = to_set(set, t + min, t, min) {
                set = Some(at);
            }
            if let Some(b) = before.filter(|b| *b > t) {
                assert!(set.is_some_and(|s| s <= b), "pushed out at {t}: {before:?} to {set:?}");
            }
        }
        assert_eq!(to_set(None, now + min, now, min), Some(now + min), "none set");
        assert_eq!(to_set(Some(now - 1), now + min, now, min), Some(now + min), "one passed (firing now)");
        assert_eq!(to_set(Some(now), now + min, now, min), Some(now + min), "one due this instant is firing");
        assert_eq!(to_set(Some(now + 5_000), now + min, now, min), Some(now + min), "one later than asked is brought in");
        // its work gone, nothing due soon (#1467's full e2e: the meter's
        // minute closed early, the next pass a day away): a sooner alarm set
        // for that work moves out to when work is due
        let day = 86_400_000;
        assert_eq!(to_set(Some(now + 21_700), now + day, now, min), Some(now + day), "an idle fragment's alarm a day away");
        assert_eq!(to_set(Some(now + 5_000), now + 10_000, now, min), Some(now + 10_000), "out to the earliest work due");
    }
}
