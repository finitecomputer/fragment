//! How long a fragment keeps its finished runs (cell/src/jobs.rs
//! `trim_runs`). Here so a host test runs the statements on the real
//! schema.
//!
//! A run with a call id is the guard that starts a call once: the same
//! `(principal, call_id)` again answers that run (`start_run`). A record's
//! triggered runs are keyed by the record (`record:<channel>:<seq>:<op>`),
//! and a retry of the post, mutation or publish that appended it starts
//! them again only where that guard is gone. So a guard of a run that ran
//! (succeeded or held) is kept for the retry window whatever the count:
//!
//! - succeeded, past the count: it keeps only its guard (key, hash,
//!   status), its input and output shed. No release replays a succeeded
//!   run, so nothing reads the input it no longer has.
//! - held, past the count: kept whole, so a replay runs it on its own
//!   input in this release and in the one before it, which replays any
//!   held or blocked row it finds. At most `HELD_RUNS_GUARDED` of them.
//! - blocked: trimmed by count as before. A blocked run never ran, so a
//!   retry after it is gone starts the call for the first time, not twice.
//!
//! The rows keep the shape and statuses every release reads: a rollback
//! needs nothing.

/// Trims finished runs past their age, and past the count kept unless the
/// run guards a call that ran, inside the window (a held one among the
/// newest `HELD_RUNS_GUARDED` held guards). Binds: the age cutoff
/// (`finished_at`), the count kept, the window's start (`created_at`),
/// the held guards kept.
///
/// A run's id follows its creation (AUTOINCREMENT, created at the clock),
/// and the trim reads every finished run once a pass (`finished_at` has
/// no index), as it always has.
pub const RUNS_TRIM_SQL: &str = "DELETE FROM runs WHERE status NOT IN ('queued', 'running')
     AND (finished_at < ? OR (id <= (SELECT MAX(id) FROM runs) - ? AND NOT (call_id IS NOT NULL AND created_at >= ?
         AND (status = 'succeeded' OR (status = 'held'
             AND id > COALESCE((SELECT h.id FROM runs h WHERE h.status = 'held' AND h.call_id IS NOT NULL ORDER BY h.id DESC LIMIT 1 OFFSET ?), 0))))))";

/// Sheds the input and output of the succeeded guard runs past the count
/// (`RUNS_TRIM_SQL` spares them), once each. Binds: the window's start,
/// the count kept.
pub const RUNS_SHED_SQL: &str = "UPDATE runs SET input = 'null', output = NULL
     WHERE status = 'succeeded' AND call_id IS NOT NULL AND created_at >= ? AND id <= (SELECT MAX(id) FROM runs) - ?
     AND (input != 'null' OR output IS NOT NULL)";

/// A held or blocked run queued again (`POST replay`), on the input it
/// keeps. Binds: the run. The same statement as the release before
/// `RUNS_SHED_SQL` (a test holds both to it): what the trim leaves, both
/// replay alike.
pub const RUN_REPLAY_SQL: &str = "UPDATE runs SET status = 'queued', attempt = attempt + 1, error = NULL, output = NULL, finished_at = NULL, launched_at = NULL
             WHERE id = ? AND status IN ('held', 'blocked') RETURNING op, attempt";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::FRAGMENT;
    use rusqlite::{params, Connection};

    const KEPT: i64 = 5;
    const HELD_KEPT: i64 = 3;
    const DAY: i64 = 24 * 3600 * 1000;
    const NOW: i64 = 100 * DAY;
    const AGE_CUTOFF: i64 = NOW - 30 * DAY;
    const WINDOW_START: i64 = NOW - 2 * DAY;
    const INPUT: &str = "{\"record\":\"a long body\"}";

    /// The replay statement of the release before this module (cell
    /// jobs.rs `replay` at 9094a72c5), verbatim: a release a rollback
    /// returns to reads these rows with it.
    const OLDER_REPLAY_SQL: &str = "UPDATE runs SET status = 'queued', attempt = attempt + 1, error = NULL, output = NULL, finished_at = NULL, launched_at = NULL
             WHERE id = ? AND status IN ('held', 'blocked') RETURNING op, attempt";

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
             VALUES ('work', 'channel', 'p', 'editor', 1, ?, ?, ?, ?, 1, ?, ?, ?, ?) RETURNING id",
            params![call, call.map(|_| "sha"), INPUT, status, output, error, at, at],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// `n` newer runs of `status`, each with a call id of its own.
    fn newer(db: &Connection, status: &str, n: i64) {
        for i in 0..n {
            run(db, Some(&format!("newer:{status}:{i}:{}", ids(db).len())), status, NOW);
        }
    }

    fn trim(db: &Connection) {
        db.execute(RUNS_TRIM_SQL, params![AGE_CUTOFF, KEPT, WINDOW_START, HELD_KEPT]).unwrap();
        db.execute(RUNS_SHED_SQL, params![WINDOW_START, KEPT]).unwrap();
    }

    fn ids(db: &Connection) -> Vec<i64> {
        let mut stmt = db.prepare("SELECT id FROM runs ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    fn kept(db: &Connection, id: i64) -> (String, Option<String>, Option<String>, Option<String>) {
        db.query_row("SELECT input, output, error, input_sha FROM runs WHERE id = ?", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
    }

    /// A replay by `sql` of run `id`: `None` when it queues nothing, else
    /// the input the queued run will run on.
    fn replayed(db: &Connection, sql: &str, id: i64) -> Option<String> {
        let queued: Option<i64> = db.query_row(sql, params![id], |r| r.get(1)).ok();
        queued.map(|_| db.query_row("SELECT input FROM runs WHERE id = ?", params![id], |r| r.get(0)).unwrap())
    }

    /// Goal: a run that ran, and keeps its call from starting twice,
    /// outlives the count for the whole retry window, whatever runs come
    /// after it. Method: a succeeded and a held guard run inside the
    /// window, a blocked one, and one without a call id, then more than
    /// the count of newer blocked runs, then the trim.
    #[test]
    fn a_guard_run_outlives_the_count_for_the_window() {
        let db = fragment();
        let succeeded = run(&db, Some("record:inbox:1:work"), "succeeded", NOW - DAY);
        let held = run(&db, Some("record:inbox:2:work"), "held", NOW - DAY);
        let blocked = run(&db, Some("record:inbox:3:work"), "blocked", NOW - DAY);
        let plain = run(&db, None, "succeeded", NOW - DAY);
        newer(&db, "blocked", KEPT * 3);
        trim(&db);
        let left = ids(&db);
        assert!(left.contains(&succeeded), "the succeeded guard run is kept: {left:?}");
        assert!(left.contains(&held), "the held guard run is kept: {left:?}");
        assert!(!left.contains(&blocked), "a blocked run, which never ran, goes with the count: {left:?}");
        assert!(!left.contains(&plain), "a run without a call id goes with the count: {left:?}");
        assert_eq!(left.len() as i64, KEPT + 2, "the count, and the two guards past it");
    }

    /// Goal: what keeps spared runs small, and what they keep. Method: a
    /// succeeded guard run past the count keeps its call id and hash but
    /// sheds its input and output; a held one keeps all of it; the newest
    /// `KEPT` keep theirs; the trim again changes nothing.
    #[test]
    fn a_spared_run_keeps_what_its_readers_need() {
        let db = fragment();
        let succeeded = run(&db, Some("c1"), "succeeded", NOW - DAY);
        let held = run(&db, Some("c2"), "held", NOW - DAY);
        newer(&db, "succeeded", KEPT);
        trim(&db);
        let shed = ("null".to_string(), None, None, Some("sha".to_string()));
        assert_eq!(kept(&db, succeeded), shed, "a succeeded run keeps its guard alone");
        let whole = (INPUT.to_string(), None, Some("its reason".to_string()), Some("sha".to_string()));
        assert_eq!(kept(&db, held), whole, "a held run is kept whole");
        let newest = *ids(&db).last().unwrap();
        assert_eq!(kept(&db, newest).0, INPUT, "the newest runs keep their input");
        assert!(kept(&db, newest).1.is_some(), "the newest runs keep their output");
        trim(&db);
        assert_eq!((kept(&db, succeeded), kept(&db, held)), (shed, whole), "a second pass changes nothing");
    }

    /// Goal: held runs kept past the count are bounded. Method: more held
    /// guard runs inside the window than `HELD_KEPT`, pushed past the
    /// count: the newest `HELD_KEPT` of them stay.
    #[test]
    fn held_guards_past_the_count_are_bounded() {
        let db = fragment();
        let held: Vec<i64> = (0..HELD_KEPT + 2).map(|i| run(&db, Some(&format!("h{i}")), "held", NOW - DAY)).collect();
        newer(&db, "succeeded", KEPT);
        trim(&db);
        let left = ids(&db);
        let (gone, stayed) = held.split_at(2);
        assert!(gone.iter().all(|id| !left.contains(id)), "the oldest held guards past the bound go: {left:?}");
        assert!(stayed.iter().all(|id| left.contains(id)), "the newest {HELD_KEPT} held guards stay: {left:?}");
    }

    /// Goal: the window is the bound. Method: a guard run older than the
    /// window, past the count, goes; one past the age cutoff goes even
    /// within the count.
    #[test]
    fn a_guard_run_goes_after_its_window() {
        for status in ["succeeded", "held"] {
            let db = fragment();
            let old = run(&db, Some("c-old"), status, WINDOW_START - 1);
            newer(&db, "succeeded", KEPT);
            trim(&db);
            assert!(!ids(&db).contains(&old), "a {status} guard run past its window goes with the count");

            let db = fragment();
            let ancient = run(&db, Some("c-ancient"), status, AGE_CUTOFF - 1);
            trim(&db);
            assert!(!ids(&db).contains(&ancient), "a {status} run past its age goes, guard or not");
        }
    }

    /// Goal: whatever the trim leaves, neither this release's replay nor
    /// the one before it queues a run on an input other than its own (a
    /// rollback admits the older one). Method: a held and a blocked guard
    /// run inside the window pushed past the count, the trim, then each
    /// replay statement; and a run of each within the count, which both
    /// replay as it was.
    #[test]
    fn every_release_replays_a_trimmed_run_on_its_own_input_or_not_at_all() {
        assert_eq!(RUN_REPLAY_SQL, OLDER_REPLAY_SQL, "this release replays as the one before it does");
        for (status, sql, reader) in [
            ("held", OLDER_REPLAY_SQL, "the older release"),
            ("blocked", OLDER_REPLAY_SQL, "the older release"),
            ("held", RUN_REPLAY_SQL, "this release"),
            ("blocked", RUN_REPLAY_SQL, "this release"),
        ] {
            let db = fragment();
            let past = run(&db, Some("c1"), status, NOW - DAY);
            newer(&db, status, KEPT);
            trim(&db);
            let newest = *ids(&db).last().unwrap();
            if let Some(input) = replayed(&db, sql, past) {
                assert_eq!(input, INPUT, "{reader} replays a {status} run past the count on its own input");
            }
            assert_eq!(replayed(&db, sql, newest).as_deref(), Some(INPUT), "{reader} replays a {status} run within the count");
        }
        // a held guard past the count is still there to replay, and replays
        let db = fragment();
        let held = run(&db, Some("c1"), "held", NOW - DAY);
        newer(&db, "blocked", KEPT);
        trim(&db);
        assert_eq!(replayed(&db, OLDER_REPLAY_SQL, held).as_deref(), Some(INPUT), "a kept held guard replays on its input");
    }
}
