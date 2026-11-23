//! How long a fragment keeps its finished runs (cell/src/jobs.rs
//! `trim_runs`). Here so a host test runs the statements on the real
//! schema.
//!
//! A run with a call id is the guard that starts a call once: the same
//! `(principal, call_id)` again answers that run (`start_run`). A record's
//! triggered runs are keyed by the record (`record:<channel>:<seq>:<op>`),
//! and a retry of the post, mutation or publish that appended it starts
//! them again only where that guard is gone. So a guard run is kept for
//! the retry window whatever the count; past the count it keeps only what
//! the guard needs (its key, hash and status), so how many runs a window
//! holds costs a few hundred bytes each, not their inputs.

/// Trims finished runs past their age, and past the count kept unless the
/// run guards a call inside the window. Binds: the age cutoff
/// (`finished_at`), the count kept, the window's start (`created_at`).
///
/// A run's id follows its creation (AUTOINCREMENT, created at the clock),
/// and the trim reads every finished run once a pass (`finished_at` has
/// no index), as it always has.
pub const RUNS_TRIM_SQL: &str = "DELETE FROM runs WHERE status NOT IN ('queued', 'running')
     AND (finished_at < ? OR (id <= (SELECT MAX(id) FROM runs) - ? AND (call_id IS NULL OR created_at < ?)))";

/// What a shed held or blocked run's error says: the run now keeps only
/// its guard. `RUN_REPLAY_SQL` refuses a run that says it.
pub const SHED_NOTE: &str =
    "this run is past the newest runs kept: only its call id is kept, so it is not replayed";

/// Sheds the input, output and error of the guard runs past the count
/// (`RUNS_TRIM_SQL` spares them), once each. Binds: the window's start,
/// the count kept.
pub const RUNS_SHED_SQL: &str = "UPDATE runs SET input = 'null', output = NULL,
     error = CASE WHEN status IN ('held', 'blocked') THEN 'this run is past the newest runs kept: only its call id is kept, so it is not replayed' END
     WHERE status NOT IN ('queued', 'running') AND call_id IS NOT NULL AND created_at >= ? AND id <= (SELECT MAX(id) FROM runs) - ?
     AND (input != 'null' OR output IS NOT NULL OR error IS NOT (CASE WHEN status IN ('held', 'blocked') THEN 'this run is past the newest runs kept: only its call id is kept, so it is not replayed' END))";

/// A held or blocked run queued again (`POST replay`), unless it was shed
/// (`SHED_NOTE`): its input is gone. Binds: the run.
pub const RUN_REPLAY_SQL: &str = "UPDATE runs SET status = 'queued', attempt = attempt + 1, error = NULL, output = NULL, finished_at = NULL, launched_at = NULL
     WHERE id = ? AND status IN ('held', 'blocked') AND error IS NOT 'this run is past the newest runs kept: only its call id is kept, so it is not replayed'
     RETURNING op, attempt";

// the statements spell the note out: they say what `SHED_NOTE` says
const _: () = assert!(const_contains(RUNS_SHED_SQL, SHED_NOTE));
const _: () = assert!(const_contains(RUN_REPLAY_SQL, SHED_NOTE));

const fn const_contains(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    let mut i = 0;
    while i + n.len() <= h.len() {
        let mut j = 0;
        while j < n.len() && h[i + j] == n[j] {
            j += 1;
        }
        if j == n.len() {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::FRAGMENT;
    use rusqlite::{params, Connection};

    const KEPT: i64 = 5;
    const DAY: i64 = 24 * 3600 * 1000;
    const NOW: i64 = 100 * DAY;
    const AGE_CUTOFF: i64 = NOW - 30 * DAY;
    const WINDOW_START: i64 = NOW - 2 * DAY;

    fn fragment() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        for step in FRAGMENT.steps {
            db.execute_batch(step.sql).unwrap();
        }
        db
    }

    /// A finished run: `call` its call id, if any, created and finished at `at`.
    fn run(db: &Connection, call: Option<&str>, status: &str, at: i64) -> i64 {
        let error = (status != "succeeded").then_some("its reason");
        let output = (status == "succeeded").then_some("{\"big\":true}");
        db.query_row(
            "INSERT INTO runs (op, via, principal, role, depth, call_id, input_sha, input, status, attempt, output, error, created_at, finished_at)
             VALUES ('work', 'channel', 'p', 'editor', 1, ?, ?, '{\"record\":\"a long body\"}', ?, 1, ?, ?, ?, ?) RETURNING id",
            params![call, call.map(|_| "sha"), status, output, error, at, at],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn trim(db: &Connection) {
        db.execute(RUNS_TRIM_SQL, params![AGE_CUTOFF, KEPT, WINDOW_START])
            .unwrap();
        db.execute(RUNS_SHED_SQL, params![WINDOW_START, KEPT])
            .unwrap();
    }

    fn ids(db: &Connection) -> Vec<i64> {
        let mut stmt = db.prepare("SELECT id FROM runs ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn kept(db: &Connection, id: i64) -> (String, Option<String>, Option<String>) {
        db.query_row(
            "SELECT input, output, error FROM runs WHERE id = ?",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    }

    /// Goal: a run that keeps a call from starting twice outlives the count
    /// trim for the whole retry window, whatever runs come after it.
    /// Method: two guard runs inside the window (a succeeded and a blocked
    /// one) and one without a call id, then more than the count of newer
    /// runs, then the trim.
    #[test]
    fn a_guard_run_outlives_the_count_for_the_window() {
        let db = fragment();
        let guard = run(&db, Some("record:inbox:1:work"), "succeeded", NOW - DAY);
        let blocked = run(&db, Some("record:inbox:2:work"), "blocked", NOW - DAY);
        let plain = run(&db, None, "succeeded", NOW - DAY);
        for _ in 0..KEPT * 3 {
            run(
                &db,
                Some(&format!("record:other:{}:work", ids(&db).len())),
                "blocked",
                NOW,
            );
        }
        trim(&db);
        let left = ids(&db);
        assert!(left.contains(&guard), "the guard run is kept: {left:?}");
        assert!(
            left.contains(&blocked),
            "the blocked guard run is kept: {left:?}"
        );
        assert!(
            !left.contains(&plain),
            "a run without a call id goes with the count: {left:?}"
        );
    }

    /// Goal: what keeps spared runs small. Method: a guard run pushed past
    /// the count keeps its call id and hash but sheds its input, output and
    /// error; the newest `KEPT` keep theirs.
    #[test]
    fn a_spared_run_keeps_only_its_guard() {
        let db = fragment();
        let succeeded = run(&db, Some("c1"), "succeeded", NOW - DAY);
        let blocked = run(&db, Some("c2"), "blocked", NOW - DAY);
        for i in 0..KEPT {
            run(&db, Some(&format!("n{i}")), "blocked", NOW);
        }
        trim(&db);
        assert_eq!(
            kept(&db, succeeded),
            ("null".to_string(), None, None),
            "a succeeded run sheds its input and output"
        );
        assert_eq!(
            kept(&db, blocked),
            ("null".to_string(), None, Some(SHED_NOTE.to_string())),
            "a blocked run's error says why its input went"
        );
        let sha: Option<String> = db
            .query_row(
                "SELECT input_sha FROM runs WHERE id = ?",
                params![blocked],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sha.as_deref(), Some("sha"), "the guard's hash stays");
        let newest = *ids(&db).last().unwrap();
        assert_eq!(
            kept(&db, newest).0,
            "{\"record\":\"a long body\"}",
            "the newest runs keep their input"
        );
        // the trim again changes nothing
        trim(&db);
        assert_eq!(
            kept(&db, blocked),
            ("null".to_string(), None, Some(SHED_NOTE.to_string()))
        );
    }

    /// Goal: the window is the bound. Method: a guard run older than the
    /// window, past the count, goes; one past the age cutoff goes even
    /// within the count.
    #[test]
    fn a_guard_run_goes_after_its_window() {
        let db = fragment();
        let old = run(&db, Some("c-old"), "succeeded", WINDOW_START - 1);
        for i in 0..KEPT {
            run(&db, Some(&format!("n{i}")), "succeeded", NOW);
        }
        trim(&db);
        assert!(
            !ids(&db).contains(&old),
            "a guard run past its window goes with the count"
        );

        let db = fragment();
        let ancient = run(&db, Some("c-ancient"), "succeeded", AGE_CUTOFF - 1);
        trim(&db);
        assert!(
            !ids(&db).contains(&ancient),
            "a run past its age goes, guard or not"
        );
    }

    /// Goal: a run whose input was shed is never replayed (it would run on
    /// an input it no longer has). Method: a blocked guard run past the
    /// count, shed, then the replay statement; and a blocked run within the
    /// count still replays.
    #[test]
    fn a_shed_run_is_not_replayed() {
        let db = fragment();
        let shed = run(&db, Some("c1"), "blocked", NOW - DAY);
        for i in 0..KEPT {
            run(&db, Some(&format!("n{i}")), "blocked", NOW);
        }
        trim(&db);
        assert_eq!(
            db.execute(
                RUN_REPLAY_SQL
                    .replace(" RETURNING op, attempt", "")
                    .as_str(),
                params![shed]
            )
            .unwrap(),
            0,
            "a shed run is not queued again"
        );
        let newest = *ids(&db).last().unwrap();
        assert_eq!(
            db.execute(
                RUN_REPLAY_SQL
                    .replace(" RETURNING op, attempt", "")
                    .as_str(),
                params![newest]
            )
            .unwrap(),
            1,
            "a kept blocked run replays"
        );
    }
}
