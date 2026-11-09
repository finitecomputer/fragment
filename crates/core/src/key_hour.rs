//! A computer's rolling hour of each shared (operator) key's charges
//! (FIN-448): what its owner's agents' calls on the key were charged, in
//! `SLOT_MS` slots kept in its `key_hour` table, so a cap its owner sets
//! on the key is checked where the computer hands out the key. A charge
//! counts from its call for 60 to 65 minutes: the hour, and the rest of
//! its slot. Bounded: a key keeps at most `SLOTS + 1` slots.

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
}
