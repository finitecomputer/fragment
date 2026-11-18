//! A shot's meter: the time it is billed for, and its row's reference.

use super::limits::KEEP_ALIVE_MS;
use super::schedule::{valid_live, Shot};

/// The time a shot is billed for: its session's, from the acquire to the
/// close; a close that was lost leaves the session open until its
/// inactivity timeout, so that is billed too (the money path fails closed).
pub fn billed_ms(elapsed_ms: u64, closed: bool) -> u64 {
    elapsed_ms.saturating_add(if closed { 0 } else { KEEP_ALIVE_MS })
}

/// The meter row's reference for one try at a card: the fragment's meter
/// key (`<name>@<incarnation>`), the live it shows, when that live was
/// wanted (a live wanted again later is another card), and the try.
pub fn meter_ref(key: &str, shot: &Shot) -> String {
    assert!(valid_live(&shot.live), "a shot names a commit");
    format!("card:{key}:{}:{}:{}", &shot.live[..12], shot.since, shot.attempt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::testing::{A, T0};
    use crate::price::{PriceBook, Priced, Usage};

    /// Goal: a shot is metered as Browser Rendering bills it: its session's
    /// time at $0.09 a browser hour (the price book's source), plus the
    /// margin, and a lost close is billed its session's inactivity timeout.
    /// Method: a typical two-second shot, an hour, and one whose close was lost.
    #[test]
    fn a_shot_is_priced_by_the_browser_hour() {
        let book = PriceBook::defaults();
        let price = |ms| book.price(&Usage::Browser { ms }).unwrap();
        assert_eq!(price(billed_ms(2_000, true)), Priced { list: 50, cost: 50, charge: 75 }, "2 s at $0.09 an hour: 50 µ$, 75 with the margin");
        assert_eq!(price(3_600_000), Priced { list: 90_000, cost: 90_000, charge: 135_000 }, "an hour lists at exactly $0.09");
        assert_eq!(billed_ms(2_000, false), 12_000);
        assert_eq!(price(billed_ms(2_000, false)).list, 300, "12 s");
        assert_eq!(billed_ms(u64::MAX, false), u64::MAX, "saturates");
        let s = Shot { live: A.into(), since: T0, attempt: 2 };
        assert_eq!(meter_ref("todo.ann@1790000000000", &s), format!("card:todo.ann@1790000000000:aaaaaaaaaaaa:{T0}:2"));
    }
}
