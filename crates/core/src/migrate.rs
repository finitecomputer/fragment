//! Versioned schemas for the platform's own Durable Objects: `Fragment`'s
//! supervisor, `Registry`, `Principal`, `Ledger` and `Computer` (an app
//! facet's SQLite is its author's). Each class declares its steps in
//! order, one SQL file each under `crates/core/migrations/<class>/`. An
//! object keeps the steps it took in its `_schema` table, one row each
//! (the step's number, name and SHA-256, whether it ran or was adopted,
//! and when), and `migrate` brings it to its class's newest step as it
//! starts, in one transaction (the cell's `schema.rs`): a step that fails
//! rolls every step back, and the object's version stays as it was.
//! Durable Objects refuse `PRAGMA user_version` (`SQLITE_AUTH` under
//! workerd, 2026-10-19), hence the table.
//!
//! An object refuses its code (`SchemaError`; every call answers
//! `schema_refused`) when it holds a version newer than its code knows
//! (a rollback to older code), a step another code declared under the same
//! number (another branch's step migrated it), or tables from before
//! versions that are not step 1's. Nothing is guessed and nothing is
//! changed: a reset is the operator's call (Paul's).
//!
//! **Objects made before versions** (all of them, until 2026-10-19) hold
//! tables and no `_schema`. Step 1 runs on them as the schema ran at every
//! start until then (`IF NOT EXISTS`: it makes the tables a later commit
//! added), and the object is adopted at version 1 only when everything it
//! then holds is step 1's, text for text (whitespace aside), or a shape
//! `Class::variants` names with the statement that makes it step 1's.
//!
//! **Adding a step** (a column, an index, a table, a table rebuilt):
//!
//! 1. Write `crates/core/migrations/<class>/<NNNN>_<name>.sql`, numbered
//!    one after the last, its name lower case with `_`. Plain SQL, no
//!    `BEGIN` or `COMMIT` (the runner's transaction holds it): `ALTER
//!    TABLE t ADD COLUMN c INTEGER NOT NULL DEFAULT 0` (a `NOT NULL`
//!    column needs a default), a `CREATE TABLE` or `CREATE INDEX`, or a
//!    rebuild (create the new table, `INSERT INTO … SELECT`, drop the old,
//!    `ALTER TABLE … RENAME TO`).
//! 2. Add a `step!` for it at the end of its class's `steps`, below.
//! 3. Run `cargo xtask schema-lock`, which appends it to
//!    `crates/core/migrations/schema.lock`, and commit both. `cargo xtask
//!    check` fails until the lock names every step as declared.
//!
//! For example, the two `fenced` columns of #369, which its
//! `cell/src/columns.rs` adds if missing as objects start, are a step of
//! the Registry's and one of the Computer's, and `columns.rs` goes. Two
//! files of one line each, `registry/0002_identities_fenced.sql` (`ALTER
//! TABLE identities ADD COLUMN fenced INTEGER NOT NULL DEFAULT 0;`) and
//! `computer/0002_agents_fenced.sql` (`ALTER TABLE agents ADD COLUMN
//! fenced INTEGER NOT NULL DEFAULT 0;`), are declared after the baselines:
//! `step!("identities_fenced", "registry/0002_identities_fenced.sql")` in
//! `REGISTRY`, and `step!("agents_fenced", "computer/0002_agents_fenced.sql")`
//! in `COMPUTER`. The baselines' `CREATE TABLE`s stay as they are.
//!
//! Never edit, renumber or remove a step once it is merged: objects hold
//! steps by number and SHA, and refuse code whose history differs. Two
//! branches that add the same number conflict in the lock; renumber the
//! later one.

use std::fmt;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// A Durable Object class's schema.
pub struct Class {
    /// The class's name, as the Worker binds it.
    pub name: &'static str,
    /// Its steps, oldest first: version `n` is `steps[n - 1]`.
    pub steps: &'static [Step],
    /// Shapes of step 1's objects that objects made before versions may
    /// hold, each with the statement that makes it step 1's.
    pub variants: &'static [Variant],
    /// The tables a life's end keeps (`remake`): the ended lives', or the
    /// one row a wipe leaves. Every other table is the life's.
    pub kept: &'static [&'static str],
}

/// One step of a class's schema: its name (the file's, after its number)
/// and its SQL.
#[derive(Clone, Copy)]
pub struct Step {
    pub name: &'static str,
    pub sql: &'static str,
}

/// An object step 1 makes, as an object made before versions holds it
/// instead (`sql`, as SQLite keeps it), and the statement that makes it
/// step 1's (`fix`).
pub struct Variant {
    pub name: &'static str,
    pub sql: &'static str,
    pub fix: &'static str,
    /// Why such objects exist: the commits that made them.
    pub why: &'static str,
}

/// The most steps a class declares: a bound on every read of `_schema`.
pub const STEPS_MAX: usize = 1000;
/// The most schema objects (tables, indexes, triggers) an object holds:
/// a bound on every read of `sqlite_master`.
pub const OBJECTS_MAX: usize = 1000;
/// The differences a refusal names; the rest it counts.
const DIFFERENCES_SHOWN_MAX: usize = 8;

macro_rules! step {
    ($name:literal, $file:literal) => {
        Step { name: $name, sql: include_str!(concat!("../migrations/", $file)) }
    };
}

pub const FRAGMENT: Class = Class {
    name: "Fragment",
    steps: &[step!("baseline", "fragment/0001_baseline.sql"), step!("listed", "fragment/0002_listed.sql"), step!("record_schedules", "fragment/0003_record_schedules.sql"), step!("search_forget", "fragment/0004_search_forget.sql")],
    variants: &[Variant {
        name: "members",
        sql: "CREATE TABLE members ( principal TEXT PRIMARY KEY, role TEXT NOT NULL, added_by TEXT NOT NULL, added_at INTEGER NOT NULL, kind TEXT, owner TEXT, people_only INTEGER NOT NULL DEFAULT 0)",
        fix: "ALTER TABLE members DROP COLUMN people_only",
        why: "people-only shares (#116, 2026-10-03) until they went (#313, 2026-10-13): unread and unwritten since, at its default",
    }],
    kept: &["ended", "ended_index", "ended_repos", "ended_agents", "ended_errors"],
};

pub const REGISTRY: Class = Class {
    name: "Registry",
    steps: &[
        step!("baseline", "registry/0001_baseline.sql"),
        step!("connection_signins", "registry/0002_connection_signins.sql"),
        step!("trial_hold_attempts", "registry/0003_trial_hold_attempts.sql"),
        step!("connection_fences", "registry/0004_connection_fences.sql"),
        step!("invites_answered", "registry/0005_invites_answered.sql"),
        step!("connection_accounts", "registry/0006_connection_accounts.sql"),
        step!("public_addresses", "registry/0007_public_addresses.sql"),
        step!("org_content", "registry/0008_org_content.sql"),
    ],
    variants: &[],
    kept: &[],
};

pub const PRINCIPAL: Class = Class {
    name: "Principal",
    steps: &[step!("baseline", "principal/0001_baseline.sql"), step!("listed", "principal/0002_listed.sql"), step!("memberships_dock", "principal/0003_memberships_dock.sql"), step!("memberships_pinned", "principal/0004_memberships_pinned.sql")],
    variants: &[],
    kept: &["wiped"],
};

pub const LEDGER: Class = Class { name: "Ledger", steps: &[step!("baseline", "ledger/0001_baseline.sql")], variants: &[], kept: &["wiped"] };

pub const COMPUTER: Class = Class { name: "Computer", steps: &[step!("baseline", "computer/0001_baseline.sql"), step!("agent_accounts", "computer/0002_agent_accounts.sql")], variants: &[], kept: &["wiped"] };

/// Every platform class, as the lock lists them.
pub const CLASSES: [&Class; 5] = [&FRAGMENT, &REGISTRY, &PRINCIPAL, &LEDGER, &COMPUTER];

/// What the runner needs of an object's SQLite: statements run, and rows
/// read, each an object of its columns. The cell's is the object's
/// `SqlStorage`; the host tests' an SQLite of their own.
pub trait Db {
    /// Runs `sql`: one statement or several.
    fn exec(&self, sql: &str) -> Result<(), String>;
    /// The rows `sql` reads.
    fn rows(&self, sql: &str) -> Result<Vec<Value>, String>;
}

/// What `migrate` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    /// The object holds every step already: nothing ran.
    Current { version: u32 },
    /// A new object: every step ran.
    Made { version: u32 },
    /// An object made before versions, found to hold step 1's tables (once
    /// the variants named were fixed) and adopted at 1; the steps after it ran.
    Adopted { fixed: Vec<&'static str>, version: u32 },
    /// Steps `from + 1` to `version` ran.
    Migrated { from: u32, version: u32 },
}

/// Why an object refuses its code. Each leaves the object as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// It holds a version newer than its code knows: newer code migrated
    /// it, and older code (a rollback) would read tables it does not know.
    Newer { class: &'static str, held: u32, known: u32 },
    /// Its step `version` is another code's (another branch's step of that
    /// number migrated it): its tables are not this code's.
    Diverged { class: &'static str, version: u32, held: String, declared: String },
    /// Made before versions, it holds what step 1 does not make, or not as
    /// step 1 makes it: it is reset, or a variant names its shape.
    ResetRequired { class: &'static str, differences: Vec<String> },
    /// A step failed: everything rolled back, its version as it was.
    StepFailed { class: &'static str, version: u32, name: &'static str, error: String },
    /// Its database refused the runner's own read or write, or `_schema`
    /// does not hold what the runner writes (corruption).
    Store { class: &'static str, error: String },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::Newer { class, held, known } => write!(
                f,
                "this {class} holds schema version {held}, newer than this code's {known}: newer code migrated it, and older code (a rollback) is refused rather than read tables it does not know; deploy code with its steps again"
            ),
            SchemaError::Diverged { class, version, held, declared } => write!(
                f,
                "this {class} ran its step {version} as {held}, not as this code's {declared}: other code (another branch's step {version}) migrated it; deploy that code, or reset it (the operator's call)"
            ),
            SchemaError::ResetRequired { class, differences } => {
                let shown: Vec<&str> = differences.iter().take(DIFFERENCES_SHOWN_MAX).map(String::as_str).collect();
                let more = differences.len() - shown.len();
                write!(f, "reset required: this {class} was made before schema versions, and it holds what step 1 does not make: {}", shown.join("; "))?;
                if more > 0 {
                    write!(f, "; and {more} more")?;
                }
                write!(f, " (nothing was changed; a reset is the operator's call)")
            }
            SchemaError::StepFailed { class, version, name, error } => {
                write!(f, "this {class}'s step {version} ({name}) failed, and every step rolled back: {error}")
            }
            SchemaError::Store { class, error } => write!(f, "this {class}'s schema versions could not be read or kept: {error}"),
        }
    }
}

/// The runner's own table: one row a step the object holds.
const SCHEMA_TABLE: &str = "CREATE TABLE IF NOT EXISTS _schema (
  version INTEGER PRIMARY KEY, name TEXT NOT NULL, sha TEXT NOT NULL, how TEXT NOT NULL, at INTEGER NOT NULL)";
/// A step the runner ran, or step 1 found and adopted.
const HOW_RAN: &str = "ran";
const HOW_ADOPTED: &str = "adopted";

/// A step as an object holds it (`_schema`'s row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub version: u32,
    pub name: String,
    pub sha: String,
    pub how: String,
    pub at: i64,
}

/// A step's SHA-256, its first 16 hex digits: what an object keeps of the
/// step it took, and the lock of each step declared.
pub fn sha(step: &Step) -> String {
    let digest = Sha256::digest(step.sql.as_bytes());
    let hex = hex::encode(digest);
    hex[..16].to_string()
}

/// Brings the object `db` is to `class`'s newest step, as it starts. The
/// caller holds one transaction around it, and rolls it back on any
/// error: a step that fails, or a refusal, leaves the object as it was.
pub fn migrate(db: &impl Db, class: &Class, now_ms: i64) -> Result<Ran, SchemaError> {
    assert!(!class.steps.is_empty() && class.steps.len() <= STEPS_MAX, "{} declares 1 to {STEPS_MAX} steps", class.name);
    let known = class.steps.len() as u32;
    db.exec(SCHEMA_TABLE).map_err(|error| SchemaError::Store { class: class.name, error })?;
    let taken = held(db, class)?;
    let version = taken.len() as u32;
    if version > known {
        return Err(SchemaError::Newer { class: class.name, held: version, known });
    }
    for h in &taken {
        let step = &class.steps[h.version as usize - 1];
        let declared = sha(step);
        if h.sha != declared {
            return Err(SchemaError::Diverged { class: class.name, version: h.version, held: format!("{} ({})", h.name, h.sha), declared: format!("{} ({declared})", step.name) });
        }
    }
    let ran = if version == known {
        Ran::Current { version }
    } else if version > 0 {
        run(db, class, version + 1, now_ms)?;
        Ran::Migrated { from: version, version: known }
    } else if objects(db, class)?.is_empty() {
        run(db, class, 1, now_ms)?;
        Ran::Made { version: known }
    } else {
        let fixed = adopt(db, class, now_ms)?;
        run(db, class, 2, now_ms)?;
        Ran::Adopted { fixed, version: known }
    };
    let after = held(db, class)?;
    assert_eq!(after.len() as u32, known, "{} holds every step once migrated", class.name);
    assert!(after.iter().zip(class.steps).all(|(h, s)| h.sha == sha(s)), "{} holds its steps as declared", class.name);
    Ok(ran)
}

/// The steps the object holds, oldest first: numbered 1 on, one each.
pub fn held(db: &impl Db, class: &Class) -> Result<Vec<Held>, SchemaError> {
    let corrupt = |error: String| SchemaError::Store { class: class.name, error };
    let rows = db.rows(&format!("SELECT version, name, sha, how, at FROM _schema ORDER BY version LIMIT {}", STEPS_MAX + 1)).map_err(corrupt)?;
    if rows.len() > STEPS_MAX {
        return Err(corrupt(format!("_schema holds more than {STEPS_MAX} rows")));
    }
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let version = row["version"].as_u64().and_then(|v| u32::try_from(v).ok());
        let (name, sha, how, at) = (row["name"].as_str(), row["sha"].as_str(), row["how"].as_str(), row["at"].as_i64());
        match (version, name, sha, how, at) {
            (Some(version), Some(name), Some(sha), Some(how), Some(at)) if version as usize == i + 1 && (how == HOW_RAN || how == HOW_ADOPTED) => {
                out.push(Held { version, name: name.to_string(), sha: sha.to_string(), how: how.to_string(), at });
            }
            _ => return Err(corrupt(format!("_schema's row {} is not a step the runner kept: {row}", i + 1))),
        }
    }
    Ok(out)
}

/// Runs steps `from` to the newest and keeps each.
fn run(db: &impl Db, class: &Class, from: u32, now_ms: i64) -> Result<(), SchemaError> {
    assert!(from >= 1, "steps are numbered from 1");
    // bounded: a class's steps (STEPS_MAX)
    for (i, step) in class.steps.iter().enumerate().skip(from as usize - 1) {
        let version = i as u32 + 1;
        db.exec(step.sql).map_err(|error| SchemaError::StepFailed { class: class.name, version, name: step.name, error })?;
        keep(db, class, version, step, HOW_RAN, now_ms)?;
    }
    Ok(())
}

/// Records that the object took step `version`, the next after those it holds.
fn keep(db: &impl Db, class: &Class, version: u32, step: &Step, how: &str, now_ms: i64) -> Result<(), SchemaError> {
    assert!(step.name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "a step's name is [a-z0-9_]+: {}", step.name);
    let store = |error: String| SchemaError::Store { class: class.name, error };
    let sha = sha(step);
    db.exec(&format!("INSERT INTO _schema (version, name, sha, how, at) VALUES ({version}, '{}', '{sha}', '{how}', {now_ms})", step.name)).map_err(store)?;
    let newest = db.rows("SELECT MAX(version) AS v FROM _schema").map_err(store)?;
    assert_eq!(newest.first().and_then(|r| r["v"].as_u64()), Some(u64::from(version)), "{}'s versions only grow, one step at a time", class.name);
    Ok(())
}

/// Adopts an object made before versions at version 1: fixes the variants
/// it holds, runs step 1 (`IF NOT EXISTS`, as every start ran it), and
/// keeps it only when the object then holds exactly what step 1 makes.
fn adopt(db: &impl Db, class: &Class, now_ms: i64) -> Result<Vec<&'static str>, SchemaError> {
    let refused = |differences: Vec<String>| SchemaError::ResetRequired { class: class.name, differences };
    let step = &class.steps[0];
    let mut fixed = Vec::new();
    let found = objects(db, class)?;
    for variant in class.variants {
        let holds = found.iter().any(|o| o.name == variant.name && normal(&o.sql) == normal(variant.sql));
        if holds {
            db.exec(variant.fix).map_err(|e| refused(vec![format!("{} could not be made step 1's ({}): {e}", variant.name, variant.fix)]))?;
            fixed.push(variant.name);
        }
    }
    db.exec(step.sql).map_err(|e| refused(vec![format!("step 1 does not apply to it: {e}")]))?;
    let differences = differences(&objects(db, class)?, &declared(step.sql));
    if !differences.is_empty() {
        return Err(refused(differences));
    }
    keep(db, class, 1, step, HOW_ADOPTED, now_ms)?;
    Ok(fixed)
}

/// A schema object as SQLite keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// `table` (a virtual table too), `index` or `trigger`.
    pub kind: String,
    pub name: String,
    /// The table an index or a trigger is on (a table's own name).
    pub table: String,
    pub sql: String,
}

/// The schema objects the object holds that are its class's: not
/// SQLite's (`sqlite_…`), the runtime's or the runner's (`_…`), nor a
/// virtual table's shadow tables (which go with it). In SQLite's order.
pub fn objects(db: &impl Db, class: &Class) -> Result<Vec<Object>, SchemaError> {
    let store = |error: String| SchemaError::Store { class: class.name, error };
    let rows = db
        .rows(&format!("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY rowid LIMIT {}", OBJECTS_MAX + 1))
        .map_err(store)?;
    if rows.len() > OBJECTS_MAX {
        return Err(store(format!("it holds more than {OBJECTS_MAX} schema objects")));
    }
    let shadows: Vec<String> = db.rows("PRAGMA table_list").map_err(store)?.iter().filter(|t| t["type"] == "shadow").filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let (Some(kind), Some(name), Some(table), Some(sql)) = (row["type"].as_str(), row["name"].as_str(), row["tbl_name"].as_str(), row["sql"].as_str()) else {
            return Err(store(format!("sqlite_master's row is not an object: {row}")));
        };
        let theirs = name.starts_with("sqlite_") || name.starts_with('_') || shadows.iter().any(|s| s == name);
        if !theirs {
            out.push(Object { kind: kind.to_string(), name: name.to_string(), table: table.to_string(), sql: sql.to_string() });
        }
    }
    Ok(out)
}

/// What `found` holds that `declared` does not, or not as it declares it,
/// and what it lacks: each difference named.
fn differences(found: &[Object], declared: &[Object]) -> Vec<String> {
    let mut out = Vec::new();
    for f in found {
        match declared.iter().find(|d| d.kind == f.kind && d.name == f.name) {
            Some(d) if normal(&d.sql) == normal(&f.sql) => {}
            Some(d) => out.push(format!("its {} {} is `{}`, not step 1's `{}`", f.kind, f.name, normal(&f.sql), normal(&d.sql))),
            None => out.push(format!("it holds {} {}, which step 1 does not make", f.kind, f.name)),
        }
    }
    for d in declared {
        if !found.iter().any(|f| f.kind == d.kind && f.name == d.name) {
            out.push(format!("it lacks step 1's {} {}", d.kind, d.name));
        }
    }
    out
}

/// SQL as compared: each run of whitespace one space.
fn normal(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The objects a step's SQL makes, as SQLite keeps their text: each
/// statement that starts a line with `CREATE`, to the `;` that ends a
/// line (a trigger's at its `END;`), without its `IF NOT EXISTS`. Comment
/// lines between statements are skipped; any other statement is one line,
/// and makes nothing (a `DROP INDEX IF EXISTS`).
pub fn declared(sql: &str) -> Vec<Object> {
    let mut out = Vec::new();
    let mut statement: Option<String> = None;
    // bounded: the step's lines
    for line in sql.lines() {
        let trimmed = line.trim();
        if statement.is_some() {
            assert!(!trimmed.starts_with("--"), "a step keeps its comments between statements: {trimmed}");
        } else if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        } else if trimmed.starts_with("CREATE ") {
            statement = Some(String::new());
        } else {
            assert!(trimmed.ends_with(';'), "a statement other than a CREATE is one line: {trimmed}");
            continue;
        }
        let text = statement.as_mut().expect("a statement begun");
        text.push_str(line);
        text.push('\n');
        let ends = if text.trim_start().starts_with("CREATE TRIGGER ") { trimmed == "END;" } else { trimmed.ends_with(';') };
        if ends {
            out.push(object(&statement.take().unwrap_or_default()));
        }
    }
    assert!(statement.is_none(), "a step ends its last statement");
    for (i, o) in out.iter().enumerate() {
        assert!(!out[..i].iter().any(|p| p.name == o.name), "a step makes {} once", o.name);
    }
    out
}

/// One `CREATE … IF NOT EXISTS <name> …;` as SQLite keeps it.
fn object(statement: &str) -> Object {
    let text = statement.trim().strip_suffix(';').expect("a statement ends with ;");
    let (head, rest) = text.split_once(" IF NOT EXISTS ").unwrap_or_else(|| panic!("a step's CREATE says IF NOT EXISTS: {text}"));
    let kind = match head {
        "CREATE TABLE" | "CREATE VIRTUAL TABLE" => "table",
        "CREATE INDEX" | "CREATE UNIQUE INDEX" => "index",
        "CREATE TRIGGER" => "trigger",
        other => panic!("a step creates tables, indexes and triggers, not: {other}"),
    };
    let name: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_').collect();
    assert!(!name.is_empty() && !name.starts_with('_') && !name.starts_with("sqlite_"), "a step names its objects [a-z][a-z0-9_]*: {rest}");
    let table = match kind {
        "table" => name.clone(),
        _ => normal(rest).split(" ON ").nth(1).and_then(|on| on.split([' ', '(']).next()).unwrap_or_default().to_string(),
    };
    Object { kind: kind.to_string(), sql: format!("{head} {rest}"), name, table }
}

/// Empties an object's life, at whatever version it is: drops every table
/// it holds but `class.kept` (the ended lives', or a wipe's mark), and
/// makes each again as the object holds it now, with its indexes and
/// triggers. A wipe's or a delete's, in the caller's one synchronous step.
/// Answers the tables made again.
pub fn remake(db: &impl Db, class: &Class) -> Result<Vec<String>, SchemaError> {
    let store = |error: String| SchemaError::Store { class: class.name, error };
    let found = objects(db, class)?;
    for kept in class.kept {
        if !found.iter().any(|o| o.kind == "table" && o.name == *kept) {
            return Err(store(format!("it lacks {kept}, a table its life's end keeps")));
        }
    }
    let life: Vec<&Object> = found.iter().filter(|o| !class.kept.contains(&o.table.as_str())).collect();
    let tables: Vec<String> = life.iter().filter(|o| o.kind == "table").map(|o| o.name.clone()).collect();
    for table in &tables {
        assert!(table.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "a table's name is [a-z0-9_]+: {table}");
        db.exec(&format!("DROP TABLE IF EXISTS {table}")).map_err(store)?;
    }
    // tables first, then what is on them, each in the order SQLite made it
    for kind in ["table", "index", "trigger"] {
        for o in life.iter().filter(|o| o.kind == kind) {
            db.exec(&o.sql).map_err(store)?;
        }
    }
    let after = objects(db, class)?;
    assert_eq!(after.len(), found.len(), "{}'s life is made again whole", class.name);
    Ok(tables)
}

#[cfg(any(test, feature = "sqlite"))]
pub mod host;

#[cfg(test)]
mod tests {
    use super::host::Lite;
    use super::*;

    fn migrated(db: &Lite, class: &Class, at: i64) -> Result<Ran, SchemaError> {
        db.transaction(|db| migrate(db, class, at))
    }

    /// A class of two steps over the fragment's baseline: a column, then
    /// an index on it (what #369's `fenced` columns are as steps).
    const STEP_TWO: Step = Step { name: "members_fenced", sql: "ALTER TABLE members ADD COLUMN fenced INTEGER NOT NULL DEFAULT 0;\n" };
    const STEP_THREE: Step = Step { name: "members_fenced_index", sql: "CREATE INDEX members_fenced ON members (fenced) WHERE fenced = 1;\n" };
    const FAILING: Step = Step { name: "fails", sql: "CREATE TABLE probe (a TEXT);\nINSERT INTO no_such_table VALUES (1);\n" };

    fn with(steps: &'static [Step]) -> Class {
        Class { name: "Fragment", steps, variants: FRAGMENT.variants, kept: FRAGMENT.kept }
    }

    /// The fragment's class as it stood at its baseline alone: version 1,
    /// whatever steps the fragment's own class has since.
    const ONE: &[Step] = &[step!("baseline", "fragment/0001_baseline.sql")];
    const TWO: &[Step] = &[step!("baseline", "fragment/0001_baseline.sql"), STEP_TWO];
    const THREE: &[Step] = &[step!("baseline", "fragment/0001_baseline.sql"), STEP_TWO, STEP_THREE];
    const BROKEN: &[Step] = &[step!("baseline", "fragment/0001_baseline.sql"), STEP_TWO, FAILING];

    /// Goal: a new object of every class runs every step and holds each
    /// as declared; started again, it runs nothing (the replay). Method:
    /// each class on a fresh SQLite, migrated twice.
    #[test]
    fn a_new_object_lands_at_the_newest_and_a_restart_runs_nothing() {
        for class in CLASSES {
            let db = Lite::new();
            let known = class.steps.len() as u32;
            assert_eq!(migrated(&db, class, 10), Ok(Ran::Made { version: known }), "{}", class.name);
            let held = held(&db, class).expect("held");
            assert_eq!(held.len() as u32, known);
            assert!(held.iter().all(|h| h.how == "ran" && h.at == 10));
            assert_eq!(migrated(&db, class, 20), Ok(Ran::Current { version: known }), "{} again", class.name);
            assert_eq!(super::held(&db, class).expect("held"), held, "{}: a restart keeps the steps as they were", class.name);
        }
    }

    /// Goal: what `declared` says step 1 makes is what SQLite keeps, for
    /// every class, text for text (the adoption compares the two).
    /// Method: each class's step 1 run on a fresh SQLite, its
    /// `sqlite_master` read back.
    #[test]
    fn declared_reads_step_one_as_sqlite_keeps_it() {
        for class in CLASSES {
            let db = Lite::new();
            db.exec(class.steps[0].sql).expect("step 1 applies");
            let found = objects(&db, class).expect("objects");
            let declared = declared(class.steps[0].sql);
            assert!(!declared.is_empty());
            assert_eq!(differences(&found, &declared), Vec::<String>::new(), "{}", class.name);
            for d in &declared {
                let f = found.iter().find(|f| f.name == d.name).expect("found");
                assert_eq!((&f.kind, &f.table), (&d.kind, &d.table), "{} {}", class.name, d.name);
            }
        }
    }

    /// Goal: a v1 object with real rows migrates to a v2 that adds a
    /// column, and then to a v3, and its rows survive with the column at
    /// its default. Method: the fragment's baseline, a member written,
    /// then the class of two and of three steps.
    #[test]
    fn a_v1_object_with_rows_migrates_and_keeps_them() {
        let db = Lite::new();
        assert_eq!(migrated(&db, &with(ONE), 1), Ok(Ran::Made { version: 1 }));
        db.exec("INSERT INTO members (principal, role, added_by, added_at) VALUES ('npub1a', 'owner', 'npub1a', 5)").expect("a member");
        assert_eq!(migrated(&db, &with(TWO), 2), Ok(Ran::Migrated { from: 1, version: 2 }));
        assert_eq!(db.count("SELECT COUNT(*) FROM members WHERE principal = 'npub1a' AND role = 'owner' AND fenced = 0"), 1, "the row survives, the column at its default");
        assert_eq!(migrated(&db, &with(THREE), 3), Ok(Ran::Migrated { from: 2, version: 3 }));
        let held = held(&db, &FRAGMENT).expect("held");
        assert_eq!(held.iter().map(|h| (h.version, h.at)).collect::<Vec<_>>(), [(1, 1), (2, 2), (3, 3)], "each step kept as it ran");
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'members_fenced'"), 1);
    }

    /// Goal: listed records' step 2 of the Fragment's and of the
    /// Principal's: an object at version 1 with rows migrates to 2, keeps
    /// its rows, and holds the tables the cell writes listed records to,
    /// as it writes them (cell/src/listed.rs, principal.rs); the restart
    /// after runs nothing; a delete's or a wipe's `remake` empties them.
    /// Method: each class's baseline alone, a row
    /// written, then its class as declared, and a listed record written
    /// and read back.
    #[test]
    fn listed_records_are_step_two_of_a_fragment_and_a_list() {
        let fragment = Lite::new();
        assert_eq!(migrated(&fragment, &with(ONE), 1), Ok(Ran::Made { version: 1 }));
        fragment.exec("INSERT INTO members (principal, role, added_by, added_at) VALUES ('npub1a', 'owner', 'npub1a', 5)").expect("a member");
        assert_eq!(FRAGMENT.steps[1].name, "listed");
        let to_two = with(&FRAGMENT.steps[..2]);
        assert_eq!(migrated(&fragment, &to_two, 2), Ok(Ran::Migrated { from: 1, version: 2 }));
        assert_eq!(fragment.count("SELECT COUNT(*) FROM members WHERE principal = 'npub1a'"), 1, "its member stays");
        fragment.exec("INSERT INTO code_channel_listed (channel, kinds, fields) VALUES ('work', '[\"turn.start\"]', '[\"asks\"]')").expect("a channel's listed");
        fragment.exec("INSERT INTO listed (channel, principal, seq, at, kind, fields, last) VALUES ('work', 'npub1a', 3, 9, 'turn.start', '{}', 3)").expect("a member's listed record");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM listed l JOIN code_channel_listed c ON c.channel = l.channel WHERE l.principal = 'npub1a'"), 1);
        assert_eq!(migrated(&fragment, &to_two, 3), Ok(Ran::Current { version: 2 }), "a restart runs nothing");
        let remade = remake(&fragment, &to_two).expect("a delete's remake");
        assert!(remade.contains(&"listed".to_string()) && remade.contains(&"code_channel_listed".to_string()), "a delete empties step 2's tables too: {remade:?}");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM listed") + fragment.count("SELECT COUNT(*) FROM code_channel_listed"), 0);

        let list = Lite::new();
        let baseline = Class { steps: &[step!("baseline", "principal/0001_baseline.sql")], ..PRINCIPAL };
        assert_eq!(migrated(&list, &baseline, 1), Ok(Ran::Made { version: 1 }));
        list.exec("INSERT INTO memberships (fragment, role, incarnation, version) VALUES ('chat--k3x9', 'owner', 1, 1)").expect("a row");
        assert_eq!(PRINCIPAL.steps[1].name, "listed");
        let to_two = Class { steps: &PRINCIPAL.steps[..2], ..PRINCIPAL };
        assert_eq!(migrated(&list, &to_two, 2), Ok(Ran::Migrated { from: 1, version: 2 }));
        list.exec("INSERT INTO listed (fragment, records) VALUES ('chat--k3x9', '[]')").expect("a row's listed records");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships m JOIN listed l ON l.fragment = m.fragment"), 1, "its row stays, beside its listed records");
        assert_eq!(migrated(&list, &to_two, 3), Ok(Ran::Current { version: 2 }));
        let remade = remake(&list, &to_two).expect("a wipe's remake");
        assert!(remade.contains(&"listed".to_string()), "a wipe empties step 2's table too: {remade:?}");
        assert_eq!(list.count("SELECT COUNT(*) FROM listed"), 0);
    }

    /// Goal: scheduled records are step 3 of the Fragment's
    /// (cell/src/schedules.rs): a fragment at version 2 with rows migrates
    /// to 3, keeps its rows and its code's cron schedules, and holds the
    /// table the cell keeps its data schedules in, as it writes and reads
    /// them (a paused one with no `next_at`); the restart after runs
    /// nothing; a delete's `remake` empties it.
    /// Method: the fragment's first two steps alone, a member and a code
    /// schedule written, then its class as declared, and data schedules
    /// written and read back.
    #[test]
    fn record_schedules_are_step_three_of_a_fragment() {
        let fragment = Lite::new();
        let known = FRAGMENT.steps.len() as u32;
        assert_eq!(migrated(&fragment, &with(&FRAGMENT.steps[..2]), 1), Ok(Ran::Made { version: 2 }));
        fragment.exec("INSERT INTO members (principal, role, added_by, added_at) VALUES ('npub1a', 'owner', 'npub1a', 5)").expect("a member");
        fragment.exec("INSERT INTO schedules (idx, op, cron, next_at) VALUES (0, 'tick', '* * * * *', 60000)").expect("a code schedule");
        assert_eq!(FRAGMENT.steps[2].name, "record_schedules");
        assert_eq!(migrated(&fragment, &FRAGMENT, 2), Ok(Ran::Migrated { from: 2, version: known }));
        let kept = fragment.count("SELECT COUNT(*) FROM members WHERE principal = 'npub1a'") + fragment.count("SELECT COUNT(*) FROM schedules");
        assert_eq!(kept, 2, "its member and its code's schedule stay");
        fragment
            .exec("INSERT INTO record_schedules (cron, channel, body, created_by, created_at, next_at) VALUES ('0 * * * *', 'tasks', '{\"kind\":\"routine\"}', 'npub1a', 5, 3600000)")
            .expect("a data schedule");
        fragment.exec("INSERT INTO record_schedules (cron, channel, body, created_by, created_at) VALUES ('0 9 * * *', 'notes', '{}', 'npub1a', 6)").expect("a paused one");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM record_schedules WHERE next_at <= 3600000"), 1, "the due one, by its index");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM record_schedules WHERE next_at IS NULL"), 1, "the paused one");
        assert_eq!(migrated(&fragment, &FRAGMENT, 3), Ok(Ran::Current { version: known }), "a restart runs nothing");
        let remade = remake(&fragment, &FRAGMENT).expect("a delete's remake");
        assert!(remade.contains(&"record_schedules".to_string()), "a delete empties its data schedules: {remade:?}");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM record_schedules"), 0);
    }

    /// Goal: the sources a push deleted, still to forget on the search
    /// service, are step 4 of a fragment (cell/src/searchable.rs): a
    /// fragment at step 3 migrates, keeps what it held, and holds a source
    /// due, which a restart keeps and a delete's remake empties. Method:
    /// its first three steps, then its class as declared.
    #[test]
    fn search_forget_is_step_four_of_a_fragment() {
        let fragment = Lite::new();
        let known = FRAGMENT.steps.len() as u32;
        assert_eq!(migrated(&fragment, &with(&FRAGMENT.steps[..3]), 1), Ok(Ran::Made { version: 3 }));
        fragment.exec("INSERT INTO members (principal, role, added_by, added_at) VALUES ('npub1a', 'owner', 'npub1a', 5)").expect("a member");
        assert_eq!(FRAGMENT.steps[3].name, "search_forget");
        assert_eq!(migrated(&fragment, &FRAGMENT, 2), Ok(Ran::Migrated { from: 3, version: known }));
        assert_eq!(fragment.count("SELECT COUNT(*) FROM members"), 1, "its member stays");
        fragment.exec("INSERT INTO search_forget (source, next_at) VALUES ('sessions/a.json', 5)").expect("a source to forget");
        fragment.exec("INSERT INTO search_forget (source, next_at) VALUES ('sessions/a.json', 6) ON CONFLICT (source) DO UPDATE SET next_at = excluded.next_at").expect("named once");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM search_forget WHERE next_at = 6"), 1);
        assert_eq!(migrated(&fragment, &FRAGMENT, 3), Ok(Ran::Current { version: known }), "a restart runs nothing");
        let remade = remake(&fragment, &FRAGMENT).expect("a delete's remake");
        assert!(remade.contains(&"search_forget".to_string()), "a delete empties it: {remade:?}");
        assert_eq!(fragment.count("SELECT COUNT(*) FROM search_forget"), 0);
    }

    /// Goal: the person's dock is step 3 of the Principal's (#587;
    /// cell/src/principal.rs): a list at version 2 with rows migrates to
    /// 3, keeps them, and puts every one on the dock, so no app leaves
    /// anyone's dock by the step; a row the cell writes as it does (no
    /// `dock` named) is on it too, and one taken off reads so; the restart
    /// after runs nothing; a wipe's `remake` keeps the column. Method: the
    /// list's first two steps alone, an archived row and one not written,
    /// then its class as declared.
    #[test]
    fn the_dock_is_step_three_of_a_list() {
        let list = Lite::new();
        let known = PRINCIPAL.steps.len() as u32;
        assert_eq!(migrated(&list, &Class { steps: &PRINCIPAL.steps[..2], ..PRINCIPAL }, 1), Ok(Ran::Made { version: 2 }));
        list.exec("INSERT INTO memberships (fragment, role, incarnation, version, archived) VALUES ('todo--k3x9', 'owner', 1, 1, 0), ('old--k3x9', 'viewer', 1, 1, 1)").expect("two rows");
        assert_eq!(PRINCIPAL.steps[2].name, "memberships_dock");
        assert_eq!(migrated(&list, &PRINCIPAL, 2), Ok(Ran::Migrated { from: 2, version: known }));
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE dock = 1"), 2, "every row is on the dock, the archived one too: its archiving is its own");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE fragment = 'old--k3x9' AND archived = 1"), 1, "its archiving stays as it was");
        list.exec("INSERT INTO memberships (fragment, role, incarnation, version, archived, searched) VALUES ('new--k3x9', 'owner', 2, 1, 0, '[]')").expect("a row as the cell writes one");
        list.exec("UPDATE memberships SET dock = 0 WHERE fragment = 'todo--k3x9'").expect("taken off");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE dock = 1"), 2, "a new row is on it; one taken off is not");
        assert_eq!(migrated(&list, &PRINCIPAL, 3), Ok(Ran::Current { version: known }), "a restart runs nothing");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE fragment = 'todo--k3x9' AND dock = 0"), 1, "and the choice holds");
        remake(&list, &PRINCIPAL).expect("a wipe's remake");
        assert!(list.columns("memberships").contains(&"dock".to_string()), "a wiped list's rows have the column still");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships"), 0);
    }

    /// Goal: the person's pins are step 4 of the Principal's (Paul,
    /// 2026-10-31; cell/src/principal.rs): a list at version 3 with rows
    /// migrates to 4, keeps them, and pins none, its archiving and its dock
    /// as they were; a row the cell writes as it does (no `pinned` named) is
    /// not pinned, and one pinned reads so; the restart after runs nothing; a
    /// wipe's `remake` keeps the column. Method: the list's first three steps
    /// alone, an archived row and one off the dock, then its class as declared.
    #[test]
    fn the_pin_is_step_four_of_a_list() {
        let list = Lite::new();
        let known = PRINCIPAL.steps.len() as u32;
        assert_eq!(migrated(&list, &Class { steps: &PRINCIPAL.steps[..3], ..PRINCIPAL }, 1), Ok(Ran::Made { version: 3 }));
        list.exec("INSERT INTO memberships (fragment, role, incarnation, version, archived, dock) VALUES ('todo--k3x9', 'owner', 1, 1, 0, 0), ('old--k3x9', 'viewer', 1, 1, 1, 1)").expect("two rows");
        assert_eq!(PRINCIPAL.steps[3].name, "memberships_pinned");
        assert_eq!(migrated(&list, &PRINCIPAL, 2), Ok(Ran::Migrated { from: 3, version: known }));
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE pinned = 0"), 2, "none is pinned by the step");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE (fragment = 'todo--k3x9' AND dock = 0) OR (fragment = 'old--k3x9' AND archived = 1)"), 2, "its archiving and its dock stay as they were");
        list.exec("INSERT INTO memberships (fragment, role, incarnation, version, archived, searched) VALUES ('new--k3x9', 'owner', 2, 1, 0, '[]')").expect("a row as the cell writes one");
        list.exec("UPDATE memberships SET pinned = 1 WHERE fragment = 'todo--k3x9'").expect("pinned");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE pinned = 1"), 1, "a new row is not pinned; one pinned is");
        assert_eq!(migrated(&list, &PRINCIPAL, 3), Ok(Ran::Current { version: known }), "a restart runs nothing");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships WHERE fragment = 'todo--k3x9' AND pinned = 1"), 1, "and the pin holds");
        remake(&list, &PRINCIPAL).expect("a wipe's remake");
        assert!(list.columns("memberships").contains(&"pinned".to_string()), "a wiped list's rows have the column still");
        assert_eq!(list.count("SELECT COUNT(*) FROM memberships"), 0);
    }

    /// Goal: #369's `fenced` columns (its `columns.rs` adds them if missing
    /// as objects start) are ordinary steps, as the module doc shows: a
    /// Registry and a Computer at their baseline, rows in them, take them
    /// and keep their rows at the column's default. Method: the two steps
    /// as the doc writes them, after each class's baseline.
    #[test]
    fn the_fenced_columns_are_ordinary_steps() {
        const REGISTRY_FENCED: &[Step] = &[step!("baseline", "registry/0001_baseline.sql"), Step { name: "identities_fenced", sql: "ALTER TABLE identities ADD COLUMN fenced INTEGER NOT NULL DEFAULT 0;\n" }];
        const COMPUTER_FENCED: &[Step] = &[step!("baseline", "computer/0001_baseline.sql"), Step { name: "agents_fenced", sql: "ALTER TABLE agents ADD COLUMN fenced INTEGER NOT NULL DEFAULT 0;\n" }];
        let registry = Lite::new();
        let baseline = Class { steps: &REGISTRY.steps[..1], ..REGISTRY };
        migrated(&registry, &baseline, 1).expect("a registry at its baseline");
        registry.exec("INSERT INTO identities (id, kind, created_at) VALUES ('npub1a', 'agent', 1)").expect("an identity");
        let fenced = Class { steps: REGISTRY_FENCED, ..REGISTRY };
        assert_eq!(migrated(&registry, &fenced, 2), Ok(Ran::Migrated { from: 1, version: 2 }));
        assert_eq!(registry.count("SELECT COUNT(*) FROM identities WHERE id = 'npub1a' AND fenced = 0"), 1);

        let computer = Lite::new();
        migrated(&computer, &Class { steps: &COMPUTER.steps[..1], ..COMPUTER }, 1).expect("a computer at its baseline");
        computer.exec("INSERT INTO agents (fragment, identity, owner, added_at) VALUES ('f', 'npub1a', 'npub1o', 1)").expect("an agent");
        let fenced = Class { steps: COMPUTER_FENCED, ..COMPUTER };
        assert_eq!(migrated(&computer, &fenced, 2), Ok(Ran::Migrated { from: 1, version: 2 }));
        assert_eq!(computer.count("SELECT COUNT(*) FROM agents WHERE fragment = 'f' AND connections = 'null' AND fenced = 0"), 1);
    }

    /// Goal: a step that fails rolls every step of that start back (the
    /// steps before it too) and leaves the version as it was; the code
    /// fixed, the next start runs them. Method: a class whose step 3
    /// fails after it made a table, from version 1.
    #[test]
    fn a_failing_step_rolls_back_and_the_version_stays() {
        let db = Lite::new();
        migrated(&db, &with(ONE), 1).expect("v1");
        let failed = migrated(&db, &with(BROKEN), 2);
        assert!(matches!(&failed, Err(SchemaError::StepFailed { version: 3, name: "fails", .. })), "{failed:?}");
        assert!(failed.unwrap_err().to_string().contains("no such table: no_such_table"));
        assert_eq!(held(&db, &FRAGMENT).expect("held").len(), 1, "still at 1: step 2 rolled back with 3");
        assert!(!db.columns("members").contains(&"fenced".to_string()), "step 2's column went with it");
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'probe'"), 0, "step 3's table too");
        assert_eq!(migrated(&db, &with(THREE), 3), Ok(Ran::Migrated { from: 1, version: 3 }), "fixed, it runs");
    }

    /// Goal: a restart in the middle of a migration (the transaction never
    /// committed) leaves the object as it was, and the next start runs
    /// the same steps again, once. Method: the migration's transaction
    /// abandoned (rolled back) after its steps ran, then run again.
    #[test]
    fn a_restart_mid_migration_runs_again_idempotently() {
        let db = Lite::new();
        migrated(&db, &with(ONE), 1).expect("v1");
        db.exec("INSERT INTO members (principal, role, added_by, added_at) VALUES ('npub1a', 'owner', 'npub1a', 5)").expect("a member");
        db.harness("BEGIN");
        assert_eq!(migrate(&db, &with(THREE), 2), Ok(Ran::Migrated { from: 1, version: 3 }));
        // the isolate dies here: what it ran never commits
        db.harness("ROLLBACK");
        assert_eq!(held(&db, &FRAGMENT).expect("held").len(), 1);
        assert_eq!(migrated(&db, &with(THREE), 4), Ok(Ran::Migrated { from: 1, version: 3 }), "the next start runs them");
        assert_eq!(migrated(&db, &with(THREE), 5), Ok(Ran::Current { version: 3 }), "and the one after, nothing");
        assert_eq!(db.count("SELECT COUNT(*) FROM members WHERE fenced = 0"), 1);
    }

    /// Goal: an object at a version newer than its code knows (a rollback
    /// to older code) is refused, loudly and typed, and nothing changes.
    /// Method: migrated to 3, then started by the code of 1.
    #[test]
    fn a_future_version_is_refused() {
        let db = Lite::new();
        migrated(&db, &with(THREE), 1).expect("v3");
        let refused = migrated(&db, &with(ONE), 2);
        assert_eq!(refused, Err(SchemaError::Newer { class: "Fragment", held: 3, known: 1 }));
        assert!(refused.unwrap_err().to_string().contains("newer than this code's 1"));
        assert_eq!(held(&db, &FRAGMENT).expect("held").len(), 3, "unchanged");
    }

    /// Goal: an object that ran another code's step under the same number
    /// (another branch's, deployed to the same preview) is refused, not
    /// read as this code's. Method: a step 2 of another text.
    #[test]
    fn another_codes_step_is_refused() {
        const OTHER: &[Step] = &[step!("baseline", "fragment/0001_baseline.sql"), Step { name: "members_note", sql: "ALTER TABLE members ADD COLUMN note TEXT;\n" }];
        let db = Lite::new();
        migrated(&db, &with(OTHER), 1).expect("the other branch's v2");
        let refused = migrated(&db, &with(THREE), 2);
        assert!(matches!(&refused, Err(SchemaError::Diverged { version: 2, .. })), "{refused:?}");
        assert!(refused.unwrap_err().to_string().contains("members_note"));
    }

    /// Goal: an object made before versions whose tables are step 1's is
    /// adopted at 1 (the tables a later commit added made for it), keeps
    /// its rows, and runs the steps after. Method: step 1 run bare, as
    /// every start ran its schema, less a table, then the class of three.
    #[test]
    fn an_object_made_before_versions_is_adopted() {
        let db = Lite::new();
        db.exec(FRAGMENT.steps[0].sql).expect("as it ran before versions");
        db.exec("DROP TABLE meter_batches").expect("a table a later commit added");
        db.exec("INSERT INTO members (principal, role, added_by, added_at) VALUES ('npub1a', 'owner', 'npub1a', 5)").expect("a member");
        assert_eq!(migrated(&db, &with(THREE), 7), Ok(Ran::Adopted { fixed: vec![], version: 3 }));
        let held = held(&db, &FRAGMENT).expect("held");
        assert_eq!(held.iter().map(|h| h.how.as_str()).collect::<Vec<_>>(), ["adopted", "ran", "ran"]);
        assert_eq!(db.count("SELECT COUNT(*) FROM members WHERE fenced = 0"), 1);
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'meter_batches'"), 1);
    }

    /// Goal: a Registry made before #367 (every deployment's, until it
    /// merged) is adopted at step 1: step 1 drops the index #367 replaced
    /// (`org_members_org_email`) and makes its tables, as every start ran
    /// it. Method: step 1 run bare, then the Registry as it stood before
    /// #367: its tables gone, the old index back.
    #[test]
    fn a_registry_from_before_org_consent_is_adopted() {
        let db = Lite::new();
        db.exec(REGISTRY.steps[0].sql).expect("as it ran before versions");
        db.exec(
            "DROP TABLE org_payers; DROP TABLE stripe_cancels; DROP TABLE wiped_orgs; DROP TABLE offer_mails; DROP TABLE offer_answers;
             DROP INDEX org_members_offer_email; CREATE UNIQUE INDEX org_members_org_email ON org_members (org, email);",
        )
        .expect("the Registry before #367");
        db.exec("INSERT INTO org_members (id, org, person, email, admin, comped, sleeps, added_at, added_by) VALUES ('m', 'o', 'npub1a', 'a@b.c', 1, 0, 0, 1, 'npub1a')").expect("a member");
        assert_eq!(migrated(&db, &REGISTRY, 7), Ok(Ran::Adopted { fixed: vec![], version: REGISTRY.steps.len() as u32 }));
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('org_members_org_email')"), 0, "the old index went");
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('org_payers', 'org_members_offer_email')"), 2);
        assert_eq!(db.count("SELECT COUNT(*) FROM org_members"), 1);
    }

    /// Goal: every object a deployment holds is adopted: each class's
    /// schema as the code each deployment ran made it, copied from git
    /// (crates/core/migrations/deployed/<commit>/<class>.sql): #271's
    /// merge (76adcb35, the reset p5 took, people-only's `members`),
    /// finite.chat's deploy (33e323cd), preview.finite.build's (035ee8e8),
    /// master's before #367 (7b84dcd3) and after it (e62d9991, the last
    /// before versions). Method: each run bare, then migrated.
    #[test]
    fn every_deployed_schema_is_adopted() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/deployed");
        let mut commits: Vec<String> = std::fs::read_dir(&root).expect("the deployed schemas").map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned()).collect();
        commits.sort();
        assert_eq!(commits, ["035ee8e8", "33e323cd", "76adcb35", "7b84dcd3", "e62d9991"]);
        for commit in &commits {
            for class in CLASSES {
                let ddl = std::fs::read_to_string(root.join(commit).join(format!("{}.sql", class.name.to_ascii_lowercase()))).expect("a class's schema");
                let db = Lite::new();
                db.exec(&ddl).expect("as it ran");
                let ran = migrated(&db, class, 7);
                let fixed: Vec<&str> = if commit == "76adcb35" && class.name == "Fragment" { vec!["members"] } else { vec![] };
                assert_eq!(ran, Ok(Ran::Adopted { fixed, version: class.steps.len() as u32 }), "{} at {commit}", class.name);
            }
        }
    }

    /// Goal: a fragment from the days of people-only shares (`members`
    /// with `people_only`) is fixed to step 1's shape, its rows kept, and
    /// adopted; the column's history is the variant's. Method: step 1 run
    /// bare, then the column added as those fragments held it.
    #[test]
    fn a_named_variant_is_fixed_then_adopted() {
        let db = Lite::new();
        db.exec(FRAGMENT.steps[0].sql).expect("as it ran before versions");
        db.exec("ALTER TABLE members ADD COLUMN people_only INTEGER NOT NULL DEFAULT 0").expect("people-only's column");
        db.exec("INSERT INTO members (principal, role, added_by, added_at, people_only) VALUES ('npub1a', 'owner', 'npub1a', 5, 1)").expect("a member");
        assert_eq!(migrated(&db, &FRAGMENT, 7), Ok(Ran::Adopted { fixed: vec!["members"], version: FRAGMENT.steps.len() as u32 }), "adopted at 1, then its later steps run");
        assert!(!db.columns("members").contains(&"people_only".to_string()));
        assert_eq!(db.count("SELECT COUNT(*) FROM members"), 1);
    }

    /// Goal: an object made before versions whose tables are not step 1's
    /// (one from before a column, as #186's lists and #178's fragments
    /// were; one with a table step 1 never makes) is refused, typed and
    /// named, and nothing changes: no step kept, no table made. Method:
    /// step 1 bare, less `runs.code`; then, apart, plus a stray table.
    #[test]
    fn an_object_that_is_not_step_ones_requires_a_reset() {
        let db = Lite::new();
        db.exec(FRAGMENT.steps[0].sql).expect("as it ran before versions");
        db.exec("ALTER TABLE runs DROP COLUMN code; DROP TABLE meter_batches").expect("a fragment from before #178");
        let refused = migrated(&db, &FRAGMENT, 7);
        let Err(SchemaError::ResetRequired { differences, .. }) = &refused else { panic!("{refused:?}") };
        assert_eq!(differences.len(), 1, "{differences:?}");
        assert!(differences[0].starts_with("its table runs is"), "{differences:?}");
        assert!(refused.unwrap_err().to_string().starts_with("reset required: this Fragment"));
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = '_schema'"), 0, "no step kept: not even the runner's table");
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'meter_batches'"), 0, "step 1's table rolled back with it");

        let stray = Lite::new();
        stray.exec(LEDGER.steps[0].sql).expect("as it ran before versions");
        stray.exec("CREATE TABLE usernames (name TEXT PRIMARY KEY)").expect("a table from before");
        let refused = migrated(&stray, &LEDGER, 7);
        assert_eq!(refused, Err(SchemaError::ResetRequired { class: "Ledger", differences: vec!["it holds table usernames, which step 1 does not make".into()] }));
    }

    /// Goal: a wipe's or a delete's remake keeps the shape the steps made
    /// (a column a later step added survives it, which `CREATE TABLE IF
    /// NOT EXISTS` from step 1 would lose), empties the life's tables,
    /// keeps the kept ones and their rows, and leaves the version as it
    /// is. Method: the class of three, rows in a life table and a kept
    /// one, then `remake`; the list's full-text table too.
    #[test]
    fn remake_empties_the_life_at_its_version() {
        let db = Lite::new();
        migrated(&db, &with(THREE), 1).expect("v3");
        db.exec("INSERT INTO members (principal, role, added_by, added_at, fenced) VALUES ('npub1a', 'owner', 'npub1a', 5, 1)").expect("a member");
        db.exec("INSERT INTO ended (incarnation, name, npub, facet, ended_at, next_at) VALUES (1, 'a', 'npub1a', 'f', 1, 1)").expect("an ended life");
        let before = objects(&db, &FRAGMENT).expect("objects");
        let tables = db.transaction(|db| remake(db, &FRAGMENT)).expect("remade");
        assert!(tables.contains(&"members".to_string()) && !tables.contains(&"ended".to_string()));
        assert_eq!(objects(&db, &FRAGMENT).expect("objects").len(), before.len());
        assert_eq!(db.count("SELECT COUNT(*) FROM members"), 0);
        assert!(db.columns("members").contains(&"fenced".to_string()), "step 2's column survives");
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'members_fenced'"), 1, "and step 3's index");
        assert_eq!(db.count("SELECT COUNT(*) FROM ended"), 1, "a kept table keeps its rows");
        assert_eq!(migrated(&db, &with(THREE), 2), Ok(Ran::Current { version: 3 }));

        let list = Lite::new();
        migrated(&list, &PRINCIPAL, 1).expect("a list");
        list.exec("INSERT INTO search_entries (fragment, n, channel, seq, at, text) VALUES ('f', 1, 'chat', 1, 1, 'kale')").expect("an entry");
        list.exec("INSERT INTO wiped (at) VALUES (9)").expect("the mark");
        list.transaction(|db| remake(db, &PRINCIPAL)).expect("remade");
        assert_eq!(list.count("SELECT COUNT(*) FROM search_text WHERE search_text MATCH 'kale'"), 0, "its search is empty");
        list.exec("INSERT INTO search_entries (fragment, n, channel, seq, at, text) VALUES ('f', 1, 'chat', 1, 1, 'chard')").expect("an entry after");
        assert_eq!(list.count("SELECT COUNT(*) FROM search_text WHERE search_text MATCH 'chard'"), 1, "and its triggers fill it again");
        assert_eq!(list.count("SELECT COUNT(*) FROM wiped"), 1);
    }

    /// Goal: a kept table the object lacks is refused before anything is
    /// dropped (a kept list that names the wrong table would otherwise
    /// drop the right one). Method: the ledger without its `wiped`.
    #[test]
    fn remake_refuses_without_its_kept_tables() {
        let db = Lite::new();
        migrated(&db, &LEDGER, 1).expect("a ledger");
        db.exec("DROP TABLE wiped; INSERT INTO meta (key, value) VALUES ('k', 'v')").expect("no mark");
        let refused = remake(&db, &LEDGER);
        assert!(matches!(&refused, Err(SchemaError::Store { .. })), "{refused:?}");
        assert_eq!(db.count("SELECT COUNT(*) FROM meta"), 1, "nothing dropped");
    }

    /// Goal: each class's declaration holds: steps named as their files
    /// are, every file under its directory declared in order, each
    /// variant a shape of a step 1 object, and each kept table one step 1
    /// makes. Method: the migrations directory, read.
    #[test]
    fn every_class_declares_its_files_in_order() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        for class in CLASSES {
            let dir = root.join(class.name.to_ascii_lowercase());
            let mut files: Vec<String> = std::fs::read_dir(&dir).expect("its directory").map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned()).collect();
            files.sort();
            let named: Vec<String> = class.steps.iter().enumerate().map(|(i, s)| format!("{:04}_{}.sql", i + 1, s.name)).collect();
            assert_eq!(files, named, "{}: one file a step, numbered from 1", class.name);
            for (file, step) in files.iter().zip(class.steps) {
                assert_eq!(std::fs::read_to_string(dir.join(file)).expect("the file"), step.sql, "{file} is the step's text");
            }
            let made = declared(class.steps[0].sql);
            for kept in class.kept {
                assert!(made.iter().any(|o| o.kind == "table" && o.name == *kept), "{}: step 1 makes {kept}", class.name);
            }
            for v in class.variants {
                let d = made.iter().find(|o| o.name == v.name).expect("a variant of step 1's object");
                assert_ne!(normal(v.sql), normal(&d.sql), "a variant differs from step 1's {}", v.name);
            }
        }
    }
}
