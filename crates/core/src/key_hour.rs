//! A computer's rolling hour of each shared (operator) key's charges
//! (FIN-448): what its owner's agents' calls on the key were charged, in
//! `SLOT_MS` slots kept in its `key_hour` table, so a cap its owner sets
//! on the key is checked where the computer hands out the key. A charge
//! counts from its call for 60 to 65 minutes: the hour, and the rest of
//! its slot. Bounded: a key keeps at most `SLOTS + 1` slots.

/// What one shared key may be charged a person in a rolling hour before
/// its calls pause (FIN-448; a decision for Paul). At a 200% margin, a
/// loop like 2026-11-07's (a 200-character clip every 1.75 s, $1.65 a
/// minute) reaches it in about three minutes, while an hour of normal use
/// fits under it: about 70 spoken replies of 300 characters, a dozen X
/// searches or ten songs.
pub const KEY_HOUR_CAP: i64 = 5 * crate::price::USD;
pub const HOUR_MS: i64 = 3_600_000;
pub const SLOT_MS: i64 = 5 * 60_000;
pub const SLOTS: i64 = HOUR_MS / SLOT_MS;

const _: () = assert!(HOUR_MS % SLOT_MS == 0, "the hour is whole slots");

/// The slot a charge at `at_ms` counts in.
pub fn slot_of(at_ms: i64) -> i64 {
    at_ms.div_euclid(SLOT_MS)
}

/// The oldest slot that counts at `now_ms`: older ones are let go.
pub fn oldest(now_ms: i64) -> i64 {
    slot_of(now_ms) - SLOTS
}

/// Whether `slot` counts at `now_ms`: one ahead of it (a clock that went
/// back) does not.
fn counts(slot: i64, now_ms: i64) -> bool {
    (oldest(now_ms)..=slot_of(now_ms)).contains(&slot)
}

/// What `slots`, `(slot, micros)`, hold that counts at `now_ms`.
pub fn spent(slots: &[(i64, i64)], now_ms: i64) -> i64 {
    slots.iter().filter(|(slot, _)| counts(*slot, now_ms)).fold(0, |sum, (_, micros)| sum.saturating_add(*micros))
}

/// A key paused at `now_ms`: what `slots` hold that counts, when it is
/// `cap` or more, and how long until it is under `cap`, as its oldest
/// slots age out.
pub fn paused(slots: &[(i64, i64)], cap: i64, now_ms: i64) -> Option<(i64, i64)> {
    let spent = spent(slots, now_ms);
    if spent < cap {
        return None;
    }
    let mut left = spent;
    for (slot, micros) in slots.iter().filter(|(slot, _)| counts(*slot, now_ms)) {
        left = left.saturating_sub(*micros);
        if left < cap {
            // it stops counting as the slot `SLOTS` after it ends
            return Some((spent, (slot + SLOTS + 1) * SLOT_MS - now_ms));
        }
    }
    unreachable!("what counts sums to `spent`, at least `cap` > 0 of it")
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_790_812_800_000;

    /// Goal: a charge counts for the hour and the rest of its slot, then
    /// goes; one from ahead of the clock counts not. Method: charges a
    /// slot apart, read as the clock moves.
    #[test]
    fn a_charge_counts_for_the_hour_and_its_slot() {
        let slots = [(slot_of(T), 3), (slot_of(T + SLOT_MS), 4)];
        assert_eq!(spent(&slots, T), 3, "the later slot is ahead of the clock");
        assert_eq!(spent(&slots, T + SLOT_MS), 7);
        let first_gone = (slot_of(T) + SLOTS + 1) * SLOT_MS;
        assert_eq!(spent(&slots, first_gone - 1), 7, "the first still counts a moment before");
        assert_eq!(spent(&slots, first_gone), 4, "then it goes");
        assert_eq!(spent(&slots, first_gone + SLOT_MS), 0);
        assert!(first_gone - T >= HOUR_MS && first_gone - T <= HOUR_MS + SLOT_MS);
    }

    /// Goal: what is kept stays bounded however fast calls come: the
    /// slots from `oldest` on are at most `SLOTS + 1`. Method: a call
    /// every 10 s for three hours, pruned as the table is.
    #[test]
    fn the_kept_slots_are_bounded() {
        let mut kept: Vec<(i64, i64)> = vec![];
        for n in 0..(3 * HOUR_MS / 10_000) {
            let now = T + n * 10_000;
            match kept.last_mut() {
                Some((slot, micros)) if *slot == slot_of(now) => *micros += 1,
                _ => kept.push((slot_of(now), 1)),
            }
            kept.retain(|(slot, _)| *slot >= oldest(now));
            assert!(kept.len() as i64 <= SLOTS + 1);
        }
        let now = T + 3 * HOUR_MS - 10_000;
        assert!((HOUR_MS / 10_000..=(HOUR_MS + SLOT_MS) / 10_000).contains(&spent(&kept, now)));
    }

    /// Goal: a key pauses at its cap, saying how long until the hour lets
    /// it go, and resumes as its oldest slots age out, one at a time.
    /// Method: $1.50 calls, three in one slot and one 20 minutes later.
    #[test]
    fn a_key_pauses_at_its_cap_until_the_hour_lets_it_go() {
        let call = 1_500_000;
        let mut slots = vec![(slot_of(T), 3 * call)];
        assert_eq!(paused(&slots, KEY_HOUR_CAP, T), None, "$4.50: under $5");
        slots.push((slot_of(T + 20 * 60_000), call));
        let now = T + 20 * 60_000;
        let first_gone = (slot_of(T) + SLOTS + 1) * SLOT_MS;
        assert_eq!(paused(&slots, KEY_HOUR_CAP, now), Some((4 * call, first_gone - now)), "$6.00: paused until the first slot goes");
        assert_eq!(paused(&slots, KEY_HOUR_CAP, first_gone), None, "then $1.50 is left");
        assert_eq!(paused(&slots, 4 * call, now), Some((4 * call, first_gone - now)), "at the cap exactly: paused");
        assert_eq!(paused(&slots, 4 * call + 1, now), None);
        // more than one slot must go: until the second goes too
        assert_eq!(paused(&slots, call, now), Some((4 * call, (slot_of(now) + SLOTS + 1) * SLOT_MS - now)));
    }
}
