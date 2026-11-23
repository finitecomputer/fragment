//! Per-minute counters for calls by callers who hold only the `public`
//! floor (visitors to a public fragment): one budget per caller and one
//! per fragment (docs/MODEL.md, Limits).

use std::collections::HashMap;

const HOUR_MS: i64 = 3_600_000;

/// One provider's calls an hour from one computer (FIN-616; 2026-11-20:
/// Google Places kept free, never at any volume): the hour it counts, and
/// its calls in it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Hourly {
    pub hour: i64,
    pub used: u32,
}

impl Hourly {
    /// Counts a call at `now_ms` against `max` an hour: refused, with the
    /// ms until the next hour, once the hour has had `max`.
    pub fn take(&mut self, max: u32, now_ms: i64) -> Result<(), i64> {
        let hour = now_ms.div_euclid(HOUR_MS);
        if hour != self.hour {
            (self.hour, self.used) = (hour, 0);
        }
        if self.used >= max {
            return Err((hour + 1) * HOUR_MS - now_ms);
        }
        self.used += 1;
        Ok(())
    }
}

pub struct Rate {
    per_caller: u32,
    per_fragment: u32,
    minute: i64,
    total: u32,
    per: HashMap<String, u32>,
}

impl Rate {
    pub fn new(per_caller: u32, per_fragment: u32) -> Rate {
        Rate { per_caller, per_fragment, minute: i64::MIN, total: 0, per: HashMap::new() }
    }

    /// Counts a call and says whether it fits this minute's budgets.
    pub fn allow(&mut self, caller: &str, now_ms: i64) -> bool {
        let minute = now_ms.div_euclid(60_000);
        if minute != self.minute {
            self.minute = minute;
            self.total = 0;
            self.per.clear();
        }
        let mine = self.per.get(caller).copied().unwrap_or(0);
        if self.total >= self.per_fragment || mine >= self.per_caller {
            return false;
        }
        self.total += 1;
        self.per.insert(caller.to_string(), mine + 1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid and invalid: `max` calls an hour go, the next is refused with
    /// the wait to the hour's end, and the next hour starts again.
    #[test]
    fn an_hour_takes_its_calls_and_no_more() {
        let mut h = Hourly::default();
        let t = 7 * HOUR_MS + 1_000;
        assert_eq!((0..3).map(|_| h.take(3, t)).collect::<Vec<_>>(), [Ok(()), Ok(()), Ok(())]);
        assert_eq!(h.take(3, t + 5_000), Err(HOUR_MS - 6_000), "the fourth waits for the hour's end");
        assert_eq!(h.take(3, 8 * HOUR_MS), Ok(()), "a new hour, a new count");
    }

    #[test]
    fn budgets() {
        let mut r = Rate::new(3, 5);
        for _ in 0..3 {
            assert!(r.allow("a", 60_000));
        }
        assert!(!r.allow("a", 60_500), "a's budget is spent");
        assert!(r.allow("b", 61_000));
        assert!(r.allow("c", 61_000));
        assert!(!r.allow("d", 61_000), "the fragment's budget is spent");
        assert!(r.allow("a", 120_000), "a new minute");
    }
}
