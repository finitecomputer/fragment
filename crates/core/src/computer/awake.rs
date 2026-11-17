//! A computer's awake time as its owner's ledger is charged it: one row an
//! interval its lifecycle metered (`Action::Meter`), each under the guard
//! of `AWAKE_ROW_MAX_MS` (FIN-511).

use crate::computer::limits::AWAKE_ROW_MAX_MS;
use crate::ledger::{Meter, MeterRow};
use crate::price::{Usage, INSTANCE};

/// The ledger batch of the computer `computer`'s awake `intervals`
/// (`[from, to)`, Unix ms, oldest first, as the DO's flush reads them), if
/// any is charged, and the intervals charged nothing: one longer than
/// `AWAKE_ROW_MAX_MS`, which no path of the lifecycle meters, so a path
/// that does is found by its log line, never billed (Paul, 2026-11-16: a
/// false charge is worse than a missed one, and an hour's clamp would
/// still bill an hour that never ran). An empty interval is neither.
///
/// The batch is named by its intervals' first and last starts; one the
/// guard left a row out of is named apart (`:guarded`), so a batch a
/// release before the guard sent with that row (its answer lost, its
/// intervals still kept) is never the same id with another body, which
/// the ledger refuses for good (#1302's review); each row keeps its
/// reference, which the ledger charges once.
pub fn awake_batch(computer: &str, intervals: &[(i64, i64)]) -> (Option<Meter>, Vec<(i64, i64)>) {
    let (Some((first, _)), Some((last, _))) = (intervals.first(), intervals.last()) else { return (None, vec![]) };
    let (charged, too_long): (Vec<_>, Vec<_>) = intervals.iter().copied().filter(|(from, to)| to > from).partition(|(from, to)| to - from <= AWAKE_ROW_MAX_MS);
    let rows: Vec<MeterRow> = charged
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
    let guarded = if too_long.is_empty() { "" } else { ":guarded" };
    let batch = (!rows.is_empty()).then(|| Meter { batch: format!("awake:{computer}:{first}-{last}{guarded}"), rows });
    (batch, too_long)
}

#[cfg(test)]
mod tests {
    use fragment_proto::ledger::{Plan, SetPlan};

    use super::*;
    use crate::computer::testing::T;
    use crate::computer::METER_EVERY_MS;
    use crate::ledger::{Ledger, Memory};
    use crate::price::PriceBook;

    /// A row as the cell made it before the guard: the one shape a charged
    /// row still has.
    fn row(from: i64, to: i64) -> MeterRow {
        MeterRow { reference: format!("awake:cmp:{from}"), usage: Usage::Awake { instance: INSTANCE.into(), ms: (to - from) as u64 }, fragment: None, agent: None, computer: Some("cmp".into()), at_ms: to }
    }

    /// A seat holder's ledger, which charges awake time.
    fn ledger() -> (Ledger, Memory) {
        let (mut l, mut s) = (Ledger::new(T), Memory::default());
        l.set_plan(&mut s, &SetPlan { id: "plan".into(), plan: Plan::Seat }, T).unwrap();
        (l, s)
    }

    /// Goal (FIN-511's guard): an awake row longer than `AWAKE_ROW_MAX_MS`
    /// is charged nothing, and is answered once, for its log line; a meter's
    /// five minutes and a sleep's last twenty are charged exactly as before,
    /// and so is one of exactly the bound, in a batch named apart. Invalid:
    /// an empty interval is neither. Method: the DO's intervals, as its
    /// flush reads them.
    #[test]
    fn an_awake_row_past_the_guard_is_charged_nothing() {
        let days = 3 * 24 * 3_600_000;
        let last = T + METER_EVERY_MS + 20 * 60_000;
        let intervals = [(T, T + METER_EVERY_MS), (T + METER_EVERY_MS, last), (last, last + days), (last + days, last + days), (T + 2 * days, T + 2 * days + AWAKE_ROW_MAX_MS), (T + 3 * days, T + 3 * days + AWAKE_ROW_MAX_MS + 1)];
        let (batch, too_long) = awake_batch("cmp", &intervals);
        let batch = batch.expect("rows to charge");
        assert_eq!(batch.rows, vec![row(T, T + METER_EVERY_MS), row(T + METER_EVERY_MS, last), row(T + 2 * days, T + 2 * days + AWAKE_ROW_MAX_MS)], "five minutes, a sleep's last twenty, and the bound: as before");
        assert_eq!(batch.batch, format!("awake:cmp:{T}-{}:guarded", T + 3 * days));
        assert_eq!(too_long, vec![(last, last + days), (T + 3 * days, T + 3 * days + AWAKE_ROW_MAX_MS + 1)], "three days, and a millisecond past the bound: nothing, each once");
        // charged: the rows within it, and nothing for the rest
        let (mut l, mut s) = ledger();
        let charged = l.meter(&mut s, &batch, T + 4 * days, &PriceBook::defaults()).expect("charged");
        assert_eq!((charged.rows_new, charged.refused.len()), (3, 0));
        // a batch the guard left nothing out of: named as it always was
        let (plain, none) = awake_batch("cmp", &intervals[..2]);
        assert_eq!((plain.map(|m| m.batch), none), (Some(format!("awake:cmp:{T}-{}", T + METER_EVERY_MS)), vec![]));
        assert_eq!(awake_batch("cmp", &[]), (None, vec![]));
        assert_eq!(awake_batch("cmp", &[(last, last + days)]).0, None, "all of it past the guard: no batch");
    }

    /// Goal (#1302's review): a full batch a release before the guard sent
    /// (an hour and a millisecond, then 63 five-minute rows), charged but
    /// its answer lost, its intervals still kept, replays through this
    /// release: accepted, never refused as another body under its id
    /// (which would hold every later flush behind it), and its rows not
    /// charged twice. Method: that release's batch, then this one's of the
    /// same intervals, on one ledger.
    #[test]
    fn a_batch_sent_before_the_guard_replays_through_it() {
        let mut intervals = vec![(T, T + AWAKE_ROW_MAX_MS + 1)];
        let mut at = T + AWAKE_ROW_MAX_MS + 1;
        for _ in 0..63 {
            intervals.push((at, at + METER_EVERY_MS));
            at += METER_EVERY_MS;
        }
        let first_last = format!("awake:cmp:{}-{}", intervals[0].0, intervals[63].0);
        let before = Meter { batch: first_last, rows: intervals.iter().map(|(from, to)| row(*from, *to)).collect() };
        let (mut l, mut s) = ledger();
        let book = PriceBook::defaults();
        let sent = l.meter(&mut s, &before, at, &book).expect("the release before charged it");
        assert_eq!(sent.rows_new, 64);
        let (now, too_long) = awake_batch("cmp", &intervals);
        let replayed = l.meter(&mut s, &now.expect("rows to charge"), at + 60_000, &book).expect("its replay is accepted");
        assert_eq!((replayed.charged, replayed.rows_new, replayed.rows_before, too_long.len()), (0, 0, 63, 1), "charged once, by its references");
    }
}
