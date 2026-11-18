//! The card tests' shared helpers.

use super::schedule::{Cards, Next, Outcome, Pace, Shot};

pub(super) const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub(super) const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
pub(super) const PACE: Pace = Pace { first_ms: 10_000, max_ms: 3_600_000 };
pub(super) const T0: i64 = 1_790_000_000_000;

pub(super) fn blob(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

pub(super) fn image(n: u8) -> Outcome {
    Outcome::Image { blob: blob(n), size: 40_000 }
}

pub(super) fn shoot(cards: &mut Cards, now: i64) -> Shot {
    match cards.next(now, PACE) {
        Next::Shoot(s) => s,
        other => panic!("expected a shot, got {other:?}"),
    }
}
