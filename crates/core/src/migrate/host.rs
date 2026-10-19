//! The host's side of the schemas (feature `sqlite`; never in the cell):
//! an object's database as a real SQLite (the bundled one), and the lock,
//! `crates/core/migrations/schema.lock`, which `cargo xtask schema-lock`
//! writes and `cargo xtask check` compares (xtask/src/schema_lock.rs).
//!
//! The lock names each class's steps (number, name, SHA) and what they
//! make on a new object, one line an object. It is the guard against a
//! schema changed without a step: a step's text edited after it was
//! locked, a step removed (#197 took three columns' migrations, and
//! objects made before them refused every statement naming them), or a
//! step the lock lacks (two branches that add the same number then
//! conflict in it).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::{json, Value};

use super::{migrate, normal, objects, sha, Class, Db, SchemaError, CLASSES};

/// An object's database on the host, refusing what a Durable Object's
/// SQLite refuses (its authorizer: `not authorized`, `SQLITE_AUTH` under
/// workerd, probed 2026-10-19): a PRAGMA but those that read (`READ_PRAGMAS`:
/// `user_version` is refused), a transaction's or a savepoint's statement (its runtime
/// holds the transaction), ATTACH and DETACH, and a TEMP object. A step
/// that names one fails here as it would there, so no lock locks it.
pub struct Lite {
    conn: rusqlite::Connection,
    /// The harness's own `BEGIN`, `COMMIT` or `ROLLBACK` is under way
    /// (`harness`): what `transactionSync` is to the cell.
    harness: Arc<AtomicBool>,
}

impl Lite {
    pub fn new() -> Lite {
        let conn = rusqlite::Connection::open_in_memory().expect("an in-memory SQLite");
        let harness = Arc::new(AtomicBool::new(false));
        let open = Arc::clone(&harness);
        conn.authorizer(Some(move |ctx: AuthContext<'_>| authorized(&ctx.action, open.load(Ordering::SeqCst)))).expect("the authorizer is set");
        Lite { conn, harness }
    }

    /// The harness's own transaction statement (`BEGIN`, `COMMIT`,
    /// `ROLLBACK`), which no object's code runs.
    pub fn harness(&self, sql: &str) {
        self.harness.store(true, Ordering::SeqCst);
        let ran = self.conn.execute_batch(sql);
        self.harness.store(false, Ordering::SeqCst);
        ran.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    /// `f` in one transaction, committed on `Ok` and rolled back on `Err`,
    /// as the cell's `transactionSync` holds `migrate`.
    pub fn transaction<T, E>(&self, f: impl FnOnce(&Lite) -> Result<T, E>) -> Result<T, E> {
        self.harness("BEGIN");
        let answer = f(self);
        self.harness(if answer.is_ok() { "COMMIT" } else { "ROLLBACK" });
        answer
    }

    /// The one integer `sql` reads.
    pub fn count(&self, sql: &str) -> i64 {
        self.conn.query_row(sql, [], |r| r.get(0)).expect(sql)
    }

    /// `table`'s columns, in order.
    pub fn columns(&self, table: &str) -> Vec<String> {
        let rows = self.rows(&format!("PRAGMA table_info({table})")).expect("columns");
        rows.iter().map(|c| c["name"].as_str().unwrap_or("").to_string()).collect()
    }
}

/// The PRAGMAs a step's SQLite here answers: those that read a schema, and
/// `data_version`, which FTS5 reads as it writes. A step sets none.
const READ_PRAGMAS: [&str; 7] = ["table_info", "table_xinfo", "table_list", "index_info", "index_list", "index_xinfo", "data_version"];

/// What a Durable Object's SQLite refuses, the harness's transaction aside.
fn authorized(action: &AuthAction<'_>, harness: bool) -> Authorization {
    let refused = match action {
        AuthAction::Pragma { pragma_name, .. } => !READ_PRAGMAS.iter().any(|p| pragma_name.eq_ignore_ascii_case(p)),
        AuthAction::Transaction { .. } => !harness,
        AuthAction::Savepoint { .. } | AuthAction::Attach { .. } | AuthAction::Detach { .. } => true,
        AuthAction::CreateTempIndex { .. } | AuthAction::CreateTempTable { .. } | AuthAction::CreateTempTrigger { .. } | AuthAction::CreateTempView { .. } => true,
        AuthAction::DropTempIndex { .. } | AuthAction::DropTempTable { .. } | AuthAction::DropTempTrigger { .. } | AuthAction::DropTempView { .. } => true,
        _ => false,
    };
    if refused {
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

impl Default for Lite {
    fn default() -> Lite {
        Lite::new()
    }
}

impl Db for Lite {
    fn exec(&self, sql: &str) -> Result<(), String> {
        self.conn.execute_batch(sql).map_err(|e| e.to_string())
    }

    fn rows(&self, sql: &str) -> Result<Vec<Value>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map([], |r| {
                let mut row = serde_json::Map::new();
                for (i, name) in names.iter().enumerate() {
                    let v = match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null | rusqlite::types::ValueRef::Blob(_) => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Real(x) => json!(x),
                        rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    };
                    row.insert(name.clone(), v);
                }
                Ok(Value::Object(row))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }
}

/// Where the lock lives, from the repo's root.
pub const LOCK_PATH: &str = "crates/core/migrations/schema.lock";

const HEADER: &str = "# The platform's Durable Object schemas (crates/core/src/migrate.rs): each
# class's steps, frozen once merged, then what they make on a new object.
# `cargo xtask schema-lock` writes it, appending new steps and refusing to
# change a locked one; `cargo xtask check` fails when it is not what the
# steps make. Never edit it by hand.
";

/// The lock's text for `classes`: each step, then each class's objects
/// once a new object has taken every step, a line each, by table.
pub fn lock(classes: &[&Class]) -> Result<String, SchemaError> {
    let mut out = String::from(HEADER);
    out.push('\n');
    for class in classes {
        for (i, step) in class.steps.iter().enumerate() {
            out.push_str(&format!("step {} {} {} {}\n", class.name, i + 1, step.name, sha(step)));
        }
    }
    for class in classes {
        let db = Lite::new();
        db.transaction(|db| migrate(db, class, 0))?;
        let mut made = objects(&db, class)?;
        let rank = |kind: &str| ["table", "index", "trigger"].iter().position(|k| *k == kind).unwrap_or(3);
        made.sort_by(|a, b| (&a.table, rank(&a.kind), &a.name).cmp(&(&b.table, rank(&b.kind), &b.name)));
        out.push('\n');
        for o in &made {
            out.push_str(&format!("{} {} {}: {}\n", class.name, o.kind, o.name, normal(&o.sql)));
        }
    }
    Ok(out)
}

/// The lock of every platform class.
pub fn platform_lock() -> Result<String, SchemaError> {
    lock(&CLASSES)
}

/// How the declared steps differ from those a lock names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// A locked step's text or name is not as the lock froze it.
    Changed(String),
    /// The lock names a step the code no longer declares.
    Removed(String),
    /// A declared step the lock does not name yet.
    Unlocked(String),
}

impl Drift {
    pub fn message(&self) -> &str {
        match self {
            Drift::Changed(m) | Drift::Removed(m) | Drift::Unlocked(m) => m,
        }
    }
}

/// The steps `lock` names, against those `classes` declare.
pub fn drift(lock: &str, classes: &[&Class]) -> Vec<Drift> {
    let locked: Vec<(&str, u32, &str, &str)> = lock
        .lines()
        .filter_map(|l| l.strip_prefix("step "))
        .filter_map(|l| match l.split(' ').collect::<Vec<_>>()[..] {
            [class, version, name, sha] => version.parse().ok().map(|v| (class, v, name, sha)),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for &(class_name, version, name, locked_sha) in &locked {
        let class = classes.iter().find(|c| c.name == class_name);
        match class.and_then(|c| c.steps.get(version as usize - 1).map(|s| (c, s))) {
            Some((_, step)) if step.name == name && sha(step) == locked_sha => {}
            Some((c, step)) => out.push(Drift::Changed(format!(
                "{} step {version} ({name}) is not as the lock froze it ({name} {locked_sha}; now {} {}): a merged step is never edited, since the objects that took it keep its SHA and refuse code whose step differs. Restore crates/core/migrations/{}/{version:04}_{name}.sql, and put the change in a new step",
                c.name,
                step.name,
                sha(step),
                c.name.to_ascii_lowercase()
            ))),
            None => out.push(Drift::Removed(format!(
                "the lock names {class_name} step {version} ({name}), which the code no longer declares: a merged step is never removed (an object that took it refuses code that lacks it). Declare it again"
            ))),
        }
    }
    for class in classes {
        for (i, step) in class.steps.iter().enumerate() {
            let version = i as u32 + 1;
            if !locked.iter().any(|&(c, v, _, _)| c == class.name && v == version) {
                out.push(Drift::Unlocked(format!(
                    "{} step {version} ({}) is not in the lock: run `cargo xtask schema-lock`, and commit {LOCK_PATH} with it",
                    class.name, step.name
                )));
            }
        }
    }
    out
}

/// The steps a base's lock names (master's, as the tree merges into it)
/// that `lock` does not name word for word: a released step edited, then
/// its line deleted by hand and locked again, or a step removed. A
/// released step is never edited or removed, whatever the tree's own
/// lock says: objects took it.
pub fn lost(base: &str, lock: &str) -> Vec<String> {
    let kept: Vec<&str> = lock.lines().filter(|l| l.starts_with("step ")).collect();
    base.lines()
        .filter(|l| l.starts_with("step ") && !kept.contains(l))
        .map(|l| format!("master's lock names `{l}`, which this tree's does not: a released step is never edited or removed (objects took it); restore it, and put the change in a new step"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{Step, FRAGMENT, LEDGER};
    use super::*;

    fn checked_in() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::fs::read_to_string(root.join(LOCK_PATH)).expect("the lock is checked in")
    }

    /// Goal: the lock checked in is what the declared steps make (what
    /// `cargo xtask check` holds the tree to). Method: rendered, compared.
    #[test]
    fn the_checked_in_lock_is_current() {
        let lock = checked_in();
        assert_eq!(drift(&lock, &CLASSES), vec![]);
        assert!(lock == platform_lock().expect("the steps apply"), "{LOCK_PATH} is stale: run `cargo xtask schema-lock`");
    }

    /// Goal: the guard fails a schema changed with no step: a column put
    /// into a locked step's `CREATE TABLE` (the edit `IF NOT EXISTS` never
    /// applied to objects made before it), a step removed (#197's), and
    /// a step not locked. Method: classes planted with each, against the
    /// lock of the classes as declared.
    #[test]
    fn the_guard_fails_a_ddl_change_with_no_step() {
        let lock = lock(&[&FRAGMENT, &LEDGER]).expect("the lock");
        let edited_sql: &'static str = Box::leak(LEDGER.steps[0].sql.replace("caps (fragment TEXT PRIMARY KEY, micros INTEGER NOT NULL)", "caps (fragment TEXT PRIMARY KEY, micros INTEGER NOT NULL, note TEXT)").into_boxed_str());
        assert_ne!(edited_sql, LEDGER.steps[0].sql, "the plant took");
        let edited_steps: &'static [Step] = Box::leak(vec![Step { name: "baseline", sql: edited_sql }].into_boxed_slice());
        let edited = Class { steps: edited_steps, ..LEDGER };
        let found = drift(&lock, &[&FRAGMENT, &edited]);
        assert!(matches!(&found[..], [Drift::Changed(m)] if m.starts_with("Ledger step 1 (baseline) is not as the lock froze it") && m.contains("0001_baseline.sql")), "{found:?}");

        let grown: &'static [Step] = Box::leak(vec![Step { name: "baseline", sql: LEDGER.steps[0].sql }, Step { name: "caps_note", sql: "ALTER TABLE caps ADD COLUMN note TEXT;\n" }].into_boxed_slice());
        let grown = Class { steps: grown, ..LEDGER };
        let longer = super::lock(&[&FRAGMENT, &grown]).expect("the lock of two steps");
        let found = drift(&longer, &[&FRAGMENT, &LEDGER]);
        assert!(matches!(&found[..], [Drift::Removed(m)] if m.starts_with("the lock names Ledger step 2 (caps_note)")), "{found:?}");

        let found = drift(&lock, &[&FRAGMENT, &grown]);
        assert!(matches!(&found[..], [Drift::Unlocked(m)] if m.contains("cargo xtask schema-lock")), "{found:?}");
        assert!(longer.contains("Ledger table caps: CREATE TABLE caps (fragment TEXT PRIMARY KEY, micros INTEGER NOT NULL, note TEXT)"), "the lock shows what the step made");

        // the tree's lock against master's: a released step edited, its
        // line deleted by hand and locked again, passes the tree's own
        // check, and not master's
        let relocked = super::lock(&[&FRAGMENT, &edited]).expect("the edited step, locked again");
        assert_eq!(drift(&relocked, &[&FRAGMENT, &edited]), vec![], "the tree's own check passes");
        let lost = lost(&lock, &relocked);
        assert!(matches!(&lost[..], [m] if m.starts_with("master's lock names `step Ledger 1 baseline ")), "{lost:?}");
        assert_eq!(super::lost(&longer, &lock).len(), 1, "a step removed is lost too");
        assert_eq!(super::lost(&lock, &longer), Vec::<String>::new(), "a step added loses none");
        assert_eq!(super::lost("", &lock), Vec::<String>::new(), "a base with no lock (before #383) holds none");
    }

    /// Goal: a step that a Durable Object's SQLite refuses (`user_version`,
    /// a transaction's statements, a savepoint, ATTACH, a TEMP table) never
    /// locks: the host's SQLite refuses it as workerd does, where rusqlite
    /// alone would run it. Method: a class with each as its step 2.
    #[test]
    fn a_step_durable_objects_refuse_never_locks() {
        let refused = [
            "PRAGMA user_version = 2;\n",
            "BEGIN;\nCREATE TABLE notes (a TEXT);\nCOMMIT;\n",
            "SAVEPOINT s;\nCREATE TABLE notes (a TEXT);\nRELEASE s;\n",
            "CREATE TEMP TABLE notes (a TEXT);\n",
            "ATTACH DATABASE ':memory:' AS other;\n",
            "PRAGMA foreign_keys = ON;\n",
        ];
        for sql in refused {
            let steps: &'static [Step] = Box::leak(vec![Step { name: "baseline", sql: LEDGER.steps[0].sql }, Step { name: "refused", sql }].into_boxed_slice());
            let class = Class { steps, ..LEDGER };
            let locked = lock(&[&class]);
            assert!(matches!(&locked, Err(SchemaError::StepFailed { version: 2, error, .. }) if error.contains("not authorized")), "{sql}: {locked:?}");
        }
        let fine: &'static [Step] = Box::leak(vec![Step { name: "baseline", sql: LEDGER.steps[0].sql }, Step { name: "caps_note", sql: "ALTER TABLE caps ADD COLUMN note TEXT;\n" }].into_boxed_slice());
        assert!(lock(&[&Class { steps: fine, ..LEDGER }]).is_ok(), "a step it takes locks");
    }
}
