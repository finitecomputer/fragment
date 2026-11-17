//! A postable channel's retention (cell/src/channels.rs `make_room`): the
//! statement that checks its running byte total against the channel as it
//! is, run on every append. Here so a host test measures it on the real
//! schema.

/// A channel's first and last record and its running total's `meta` row
/// (`"bytes first last"`), binds: the channel, the channel, the row's key.
/// Each extremum is a subquery of its own: SQLite reads one index entry
/// for a lone `MIN` or `MAX`, and every entry of the channel for both in
/// one aggregate (review of #1059).
pub const POSTED_SPAN_SQL: &str = "SELECT (SELECT MIN(seq) FROM records WHERE channel = ?) AS lo, (SELECT MAX(seq) FROM records WHERE channel = ?) AS hi, (SELECT value FROM meta WHERE key = ?) AS kept";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::FRAGMENT;
    use rusqlite::{params, Connection, StatementStatus};

    /// Goal: the check every append makes reads a few index entries, not
    /// the channel. Method: the fragment's schema, a channel of 100,000
    /// records and its total, then the statement run once: its virtual
    /// machine steps, against the combined aggregate it replaced.
    #[test]
    fn span_reads_endpoints_not_the_channel() {
        let db = Connection::open_in_memory().unwrap();
        for step in FRAGMENT.steps {
            db.execute_batch(step.sql).unwrap();
        }
        let tx = db.unchecked_transaction().unwrap();
        {
            let mut add = tx.prepare("INSERT INTO records (channel, seq, at, principal, kind, body) VALUES ('work', ?, 0, 'p', 'k', '{}')").unwrap();
            for seq in 1..=100_000i64 {
                add.execute(params![seq]).unwrap();
            }
        }
        tx.execute("INSERT INTO meta (key, value) VALUES ('posted_bytes:work', '200000 1 100000')", []).unwrap();
        tx.commit().unwrap();
        let steps = |sql: &str| -> (i32, Vec<Option<String>>) {
            let mut stmt = db.prepare(sql).unwrap();
            let row = stmt
                .query_row(params!["work", "work", "posted_bytes:work"], |r| Ok(vec![r.get::<_, Option<i64>>(0)?.map(|n| n.to_string()), r.get::<_, Option<i64>>(1)?.map(|n| n.to_string()), r.get(2)?]))
                .unwrap();
            (stmt.get_status(StatementStatus::VmStep), row)
        };
        let (ours, row) = steps(POSTED_SPAN_SQL);
        assert_eq!(row, [Some("1".into()), Some("100000".into()), Some("200000 1 100000".into())]);
        let (combined, _) = steps("SELECT MIN(seq) AS lo, MAX(seq) AS hi, (SELECT value FROM meta WHERE key = ?3) AS kept FROM records WHERE channel = ?1 AND ?2 IS NOT NULL");
        assert!(ours < 200, "the span takes {ours} steps on 100,000 records");
        assert!(combined > 100_000, "the combined aggregate walks the channel ({combined} steps): the test measures what it means to");
    }
}
