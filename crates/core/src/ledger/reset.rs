//! An operator's reset of this month's usage (docs/ledger.md, "A month's
//! usage reset"): on one ledger, `Ledger::reset_month`; on every person's
//! and org's, a run the registry keeps, a row a payer, which its alarm
//! resets a batch at a time (cell/src/registry/resets.rs). Its bounds.

/// Payers one pass of the registry's alarm resets, each one call to its
/// ledger in turn: a pass stays far inside a Worker's subrequests, and the
/// alarm runs again at once while any is due.
pub const BATCH: u32 = 25;
/// The tries a payer gets before its run gives up on it, saying why.
pub const TRIES: i64 = 5;
/// The wait after a failed try, doubling each time: 10 s, 20 s, 40 s and
/// 80 s, two and a half minutes before a payer is given up on.
pub const RETRY_MS: i64 = 10_000;
/// The runs a registry keeps, the newest: an older one's rows go as a run
/// starts. The operators' log keeps every run's record.
pub const RUNS_KEPT: i64 = 10;
/// The failures a run's view names; it counts the rest.
pub const FAILURES_SHOWN: i64 = 100;

/// A run's id: 1 to 128 of A-Z, a-z, 0-9, `-`, `_`, `.` and `:`, so it
/// names the run in a path as it is, and its ledgers' command,
/// `reset:<id>`, stays inside a ledger's id limit.
pub fn valid_run_id(id: &str) -> bool {
    (1..=RUN_ID_MAX_BYTES).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

/// A run's id, at most.
pub const RUN_ID_MAX_BYTES: usize = 128;

const _: () = assert!(RUN_ID_MAX_BYTES + "reset:".len() <= super::ID_MAX_BYTES, "a run's ledger command is a valid id");

/// The wait before a payer's next try, after `tries` failed ones.
pub fn retry_after_ms(tries: i64) -> i64 {
    RETRY_MS.saturating_mul(1 << tries.clamp(0, TRIES))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payer_is_tried_for_two_and_a_half_minutes_before_its_run_gives_up() {
        let waits: Vec<i64> = (0..TRIES - 1).map(retry_after_ms).collect();
        assert_eq!(waits, [10_000, 20_000, 40_000, 80_000]);
        assert_eq!(waits.iter().sum::<i64>(), 150_000);
        assert_eq!(retry_after_ms(-1), RETRY_MS, "a negative count waits the least");
        assert_eq!(retry_after_ms(i64::MAX), RETRY_MS << TRIES, "the wait is bounded");
    }

    #[test]
    fn a_runs_id_names_it_in_a_path_as_it_is() {
        let longest = "r".repeat(RUN_ID_MAX_BYTES);
        for ok in ["reset-0a1b", "cli-00000000deadbeef", "a.b_c:d", longest.as_str()] {
            assert!(valid_run_id(ok), "{ok}");
            assert!(crate::price::printable(&format!("reset:{ok}"), super::super::ID_MAX_BYTES), "its ledger command's id is valid: {ok}");
        }
        let long = "r".repeat(RUN_ID_MAX_BYTES + 1);
        for bad in ["", "a/b", "a b", "a?b", "a#b", "%2F", "é", long.as_str()] {
            assert!(!valid_run_id(bad), "{bad:?}");
        }
    }
}
