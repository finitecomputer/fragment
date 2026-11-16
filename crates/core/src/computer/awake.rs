//! A computer's awake time as its owner's ledger is charged it: one row an
//! interval its lifecycle metered (`Action::Meter`), each under the guard
//! of `AWAKE_ROW_MAX_MS` (FIN-511).

use crate::computer::limits::AWAKE_ROW_MAX_MS;
use crate::ledger::MeterRow;
use crate::price::{Usage, INSTANCE};

/// The ledger rows of the computer `computer`'s awake `intervals`
/// (`[from, to)`, Unix ms, as the DO keeps them), and the intervals
/// charged nothing: one longer than `AWAKE_ROW_MAX_MS`, which no path of
/// the lifecycle meters, so a path that does is found by its log line,
/// never billed (Paul, 2026-11-16: a false charge is worse than a missed
/// one, and an hour's clamp would still bill an hour that never ran). An
/// empty interval is neither.
pub fn awake_rows(computer: &str, intervals: &[(i64, i64)]) -> (Vec<MeterRow>, Vec<(i64, i64)>) {
    let (charged, too_long): (Vec<_>, Vec<_>) = intervals.iter().copied().filter(|(from, to)| to > from).partition(|(from, to)| to - from <= AWAKE_ROW_MAX_MS);
    let rows = charged
        .into_iter()
        .map(|(from, to)| MeterRow {
            reference: format!("awake:{computer}:{from}"),
            usage: Usage::Awake { instance: INSTANCE.into(), ms: (to - from) as u64 },
            fragment: None,
            agent: None,
            computer: Some(computer.to_string()),
            at_ms: to,
        })
        .collect();
    (rows, too_long)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::testing::T;
    use crate::computer::METER_EVERY_MS;

    /// A row as the cell made it before the guard: the one shape a charged
    /// row still has.
    fn row(from: i64, to: i64) -> MeterRow {
        MeterRow { reference: format!("awake:cmp:{from}"), usage: Usage::Awake { instance: INSTANCE.into(), ms: (to - from) as u64 }, fragment: None, agent: None, computer: Some("cmp".into()), at_ms: to }
    }

    /// Goal (FIN-511's guard): an awake row longer than `AWAKE_ROW_MAX_MS`
    /// is charged nothing, and is answered once, for its log line; a meter's
    /// five minutes and a sleep's last twenty are charged exactly as before,
    /// and so is one of exactly the bound. Invalid: an empty interval is
    /// neither. Method: the DO's intervals, as its flush reads them.
    #[test]
    fn an_awake_row_past_the_guard_is_charged_nothing() {
        let days = 3 * 24 * 3_600_000;
        let last = T + METER_EVERY_MS + 20 * 60_000;
        let intervals = [(T, T + METER_EVERY_MS), (T + METER_EVERY_MS, last), (last, last + days), (last + days, last + days), (T + 2 * days, T + 2 * days + AWAKE_ROW_MAX_MS), (T + 3 * days, T + 3 * days + AWAKE_ROW_MAX_MS + 1)];
        let (rows, too_long) = awake_rows("cmp", &intervals);
        assert_eq!(rows, vec![row(T, T + METER_EVERY_MS), row(T + METER_EVERY_MS, last), row(T + 2 * days, T + 2 * days + AWAKE_ROW_MAX_MS)], "five minutes, a sleep's last twenty, and the bound: as before");
        assert_eq!(too_long, vec![(last, last + days), (T + 3 * days, T + 3 * days + AWAKE_ROW_MAX_MS + 1)], "three days, and a millisecond past the bound: nothing, each once");
        assert_eq!(awake_rows("cmp", &[]), (vec![], vec![]));
    }
}
