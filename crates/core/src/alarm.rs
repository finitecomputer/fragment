//! An object's one alarm (a Durable Object has one): when to set it.

/// The time to set the alarm to, given the time `set` it is set to now, if
/// any, and the earliest `at` its due work asks for, at `now`: `None` when
/// the alarm already set is still to come and no later than `at`, so it
/// stays. Arms one after another (a burst of calls, each with work due at
/// once) then never push a coming alarm out, each a moment past the last,
/// past the burst's end; an earlier one firing is harmless, as its pass
/// does what is due and arms again.
pub fn to_set(set: Option<i64>, at: i64, now: i64) -> Option<i64> {
    match set {
        Some(set) if set > now && set <= at => None,
        _ => Some(at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a burst of calls never starves what is due (the review of
    /// #1460: arms every 40 ms each moved a 50 ms alarm out by 40 ms, so
    /// the first queued run waited for the burst's end). Valid: a coming
    /// alarm no later than asked stays; none set, one passed, or one later
    /// than asked is set to what is asked.
    #[test]
    fn a_coming_alarm_is_never_pushed_out() {
        let now = 1_000;
        assert_eq!(to_set(Some(now + 50), now + 90, now), None, "the next arm of a burst keeps it");
        assert_eq!(to_set(Some(now + 50), now + 50, now), None);
        // a burst: an arm every 40 ms, each asking 50 ms out: an alarm still
        // to come never moves later
        let mut set: Option<i64> = None;
        for k in 0..10 {
            let t = now + 40 * k;
            let before = set;
            if let Some(at) = to_set(set, t + 50, t) {
                set = Some(at);
            }
            if let Some(b) = before.filter(|b| *b > t) {
                assert!(set.is_some_and(|s| s <= b), "pushed out at {t}: {before:?} to {set:?}");
            }
        }
        assert_eq!(to_set(None, now + 50, now), Some(now + 50), "none set");
        assert_eq!(to_set(Some(now - 1), now + 50, now), Some(now + 50), "one passed (firing now)");
        assert_eq!(to_set(Some(now), now + 50, now), Some(now + 50), "one due this instant is firing");
        assert_eq!(to_set(Some(now + 5_000), now + 50, now), Some(now + 50), "one later than asked is brought in");
    }
}
