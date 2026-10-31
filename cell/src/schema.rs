//! Each platform Durable Object's schema, made current as it starts
//! (fragment_core::migrate; crates/core/migrations): its steps run in one
//! `transactionSync`, all or none. An object that refuses its code's
//! schema (a newer version, another code's step, or tables from before
//! versions that are not step 1's) answers every call `schema_refused`,
//! naming itself and why, but those that remove everything anyway: a
//! fragment's delete, and a wipe's end of a fragment, list, ledger or
//! computer (a refused Registry answers nothing: its recovery is the
//! operator's). A fragment's delete ends in its storage's release, which
//! makes it current. A refused object's alarm looks again only where newer
//! code could take it (`Refusal::lasting`). Its sockets are none: a deploy
//! closes them, and a refused object accepts no new one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use fragment_core::migrate::{self, Class, Db, Ran, SchemaError, Step};
use fragment_proto::ErrorCode;
use serde_json::{json, Value};
use worker::wasm_bindgen::JsValue;
use worker::*;

use crate::error::{CellError, CellResult};
use crate::js;

/// How soon a refused object's alarm looks again where newer code may take
/// it: once a rollback is rolled forward, its due work waits this long at
/// most.
pub(crate) const REFUSED_ALARM_MS: i64 = 300_000;

/// An object's SQLite, as the runner reads it.
struct Sql(SqlStorage);

impl Db for Sql {
    fn exec(&self, sql: &str) -> std::result::Result<(), String> {
        self.0.exec(sql, None).map(|_| ()).map_err(|e| e.to_string())
    }

    fn rows(&self, sql: &str) -> std::result::Result<Vec<Value>, String> {
        self.0.exec(sql, None).and_then(|c| c.to_array()).map_err(|e| e.to_string())
    }
}

/// Why an object refuses its code's schema.
struct Refusal {
    /// What it answers: its class and id, and why.
    why: String,
    /// No code takes it as it is (reset required): only the operator's
    /// reset can, so nothing looks again. A newer version, another code's
    /// step, or a step that failed, newer code (or the same, restarted)
    /// may take.
    lasting: bool,
}

/// What an object's start made of its schema: ready, or the refusal it
/// answers every call with.
pub(crate) struct Schema {
    refused: RefCell<Option<Refusal>>,
}

impl Schema {
    /// The object `raw` is (its class's; `id`, as its refusal names it)
    /// made current as it starts.
    pub(crate) fn start(raw: &JsValue, sql: SqlStorage, class: &'static Class, id: &str) -> Schema {
        match migrated(raw, sql, class, class.steps, id) {
            Ok(Ran::Current { .. } | Ran::Made { .. }) => Schema { refused: RefCell::new(None) },
            Ok(ran) => {
                console_log!("{}", json!({ "schema": class.name, "object": id, "ran": said(&ran) }));
                Schema { refused: RefCell::new(None) }
            }
            Err(r) => {
                console_error!("{}", json!({ "schema": class.name, "object": id, "refused": r.why, "lasting": r.lasting }));
                Schema { refused: RefCell::new(Some(r)) }
            }
        }
    }

    /// Ok, or the refusal the object answers.
    pub(crate) fn ready(&self) -> CellResult<()> {
        match &*self.refused.borrow() {
            None => Ok(()),
            Some(r) => Err(CellError::new(ErrorCode::SchemaRefused, r.why.clone())),
        }
    }

    pub(crate) fn refused(&self) -> bool {
        self.refused.borrow().is_some()
    }

    /// Whether the object's `wiped` table holds its mark (a wiped list's,
    /// ledger's or computer's), read as it starts: a current object's
    /// always reads, and a refused one that cannot read it is not wiped.
    pub(crate) fn wiped(&self, sql: &SqlStorage) -> bool {
        let marks = sql.exec("SELECT COUNT(*) AS n FROM wiped", None).and_then(|c| c.to_array::<Value>());
        match marks {
            Ok(rows) => rows.first().and_then(|r| r["n"].as_i64()).expect("COUNT answers a row") > 0,
            Err(e) => {
                assert!(self.refused(), "a current object's wiped row reads: {e}");
                false
            }
        }
    }

    /// The object made again empty (its storage released whole, `_schema`
    /// with it) takes every step: current from now, whatever it refused.
    pub(crate) fn made_again(&self, raw: &JsValue, sql: SqlStorage, class: &'static Class, id: &str) -> CellResult<()> {
        match migrated(raw, sql, class, class.steps, id) {
            Ok(_) => {
                self.refused.replace(None);
                Ok(())
            }
            Err(r) => Err(CellError::new(ErrorCode::SchemaRefused, r.why)),
        }
    }

    /// A refused object's alarm: it says so and, where newer code may take
    /// it, wakes again within `REFUSED_ALARM_MS`, or at `due` (work it
    /// still does: a fragment's ended lives) when sooner, never later than
    /// an alarm already set. Answers whether it was refused.
    pub(crate) async fn refused_alarm(&self, state: &State, due: Option<i64>) -> Result<bool> {
        let (why, lasting) = match &*self.refused.borrow() {
            None => return Ok(false),
            Some(r) => (r.why.clone(), r.lasting),
        };
        console_error!("{}", json!({ "schema": "alarm.refused", "refused": why, "lasting": lasting }));
        let again = (!lasting).then(|| js::now_ms() + REFUSED_ALARM_MS);
        let set = state.storage().get_alarm().await?;
        let at = [due, again, set].into_iter().flatten().min();
        if let Some(at) = at.filter(|at| set != Some(*at)) {
            state.storage().set_alarm(ScheduledTime::new(js_sys::Date::new(&JsValue::from_f64(at as f64)))).await?;
        }
        Ok(true)
    }
}

/// `steps` taken in one transaction: every one, or none. A refusal names
/// the object (its class and id), and says whether it lasts.
fn migrated(raw: &JsValue, sql: SqlStorage, class: &'static Class, steps: &'static [Step], id: &str) -> std::result::Result<Ran, Refusal> {
    let named = format!("{} {id}", class.name);
    let class = Class { name: class.name, steps, variants: class.variants, kept: class.kept };
    let lasting = Rc::new(Cell::new(false));
    let lasts = Rc::clone(&lasting);
    let ran = js::transaction_sync(raw, move || {
        migrate::migrate(&Sql(sql), &class, js::now_ms()).map_err(|e| {
            lasts.set(matches!(e, SchemaError::ResetRequired { .. }));
            CellError::new(ErrorCode::SchemaRefused, format!("{named}: {e}"))
        })
    });
    ran.map_err(|e| Refusal { why: e.message, lasting: lasting.get() })
}

/// What ran, as a log line and a lever say it.
fn said(ran: &Ran) -> String {
    match ran {
        Ran::Current { version } => format!("nothing: at {version}"),
        Ran::Made { version } => format!("made at {version}"),
        Ran::Adopted { fixed, version } => format!("adopted at 1 (fixed: {fixed:?}), at {version}"),
        Ran::Migrated { from, version } => format!("migrated from {from} to {version}"),
    }
}

/// Empties the object's life at its version (`migrate::remake`): a wipe's
/// or a delete's, in the caller's one synchronous step.
pub(crate) fn remake(sql: SqlStorage, class: &'static Class) -> CellResult<Vec<String>> {
    migrate::remake(&Sql(sql), class).map_err(|e| CellError::host(e.to_string()))
}

/// The levers' view of an object's schema (fleets with test hooks): the
/// steps it holds (none from before versions, with no `_schema`), and how
/// many its code declares.
pub(crate) fn view(sql: SqlStorage, class: &'static Class) -> CellResult<Value> {
    let versioned: Vec<Value> = sql.exec("SELECT 1 FROM sqlite_master WHERE name = '_schema'", None)?.to_array()?;
    if versioned.is_empty() {
        return Ok(json!({ "class": class.name, "known": class.steps.len(), "steps": [] }));
    }
    let held = migrate::held(&Sql(sql), class).map_err(|e| CellError::new(ErrorCode::SchemaRefused, e.to_string()))?;
    let steps: Vec<Value> = held.iter().map(|h| json!({ "version": h.version, "name": h.name, "sha": h.sha, "how": h.how, "at": h.at })).collect();
    Ok(json!({ "class": class.name, "known": class.steps.len(), "steps": steps }))
}

/// A fragment's schema levers (fleets with test hooks; the e2e's
/// `migrations` section). They answer on a fragment that refuses its
/// schema too, and make the objects the runner must tell apart:
///
/// - `schema`: its steps (`view`), and the columns of `members` and `runs`;
/// - `schema-rerun`: its migration run again, as a start runs it;
/// - `schema-unversion`: an object from before versions (no `_schema`,
///   and only step 1's objects: the tables and indexes later steps made
///   go), as `people-only` one from the days of people-only shares, as
///   `before-code` one from before `runs.code` (#178);
/// - `schema-try`: its steps and one `step` more of the e2e's, `adds` a
///   column to `members`, or `fails` once it has made a table: a code's
///   newer step, which leaves the object at a version its code does not know.
pub(crate) fn fragment_lever(raw: &JsValue, sql: SqlStorage, id: &str, op: &str, shape: Option<&str>) -> CellResult<Value> {
    let class = &migrate::FRAGMENT;
    let answer = |r: std::result::Result<Ran, Refusal>| match r {
        Ok(ran) => json!({ "ran": said(&ran) }),
        Err(r) => json!({ "refused": r.why, "lasting": r.lasting }),
    };
    let columns = |sql: &SqlStorage, table: &str| -> CellResult<Vec<Value>> {
        let rows: Vec<Value> = sql.exec(&format!("PRAGMA table_info({table})"), None)?.to_array()?;
        Ok(rows.iter().map(|c| c["name"].clone()).collect())
    };
    match (op, shape) {
        ("schema", None) => {
            let mut view = view(sql.clone(), class)?;
            view["members"] = json!(columns(&sql, "members")?);
            view["runs"] = json!(columns(&sql, "runs")?);
            Ok(view)
        }
        ("schema-rerun", None) => Ok(answer(migrated(raw, sql, class, class.steps, id))),
        ("schema-unversion", _) => {
            let plant = match shape {
                None => "",
                Some("people-only") => "ALTER TABLE members ADD COLUMN people_only INTEGER NOT NULL DEFAULT 0;",
                Some("before-code") => "ALTER TABLE runs DROP COLUMN code;",
                Some(other) => return Err(CellError::invalid(format!("no shape {other}: people-only or before-code"))),
            };
            // an object from before versions holds step 1's objects alone:
            // the tables and indexes later steps made go first, and their
            // columns (each undone by name below)
            let first = migrate::declared(class.steps[0].sql);
            let held = migrate::objects(&Sql(sql.clone()), class).map_err(|e| CellError::host(e.to_string()))?;
            for o in held.iter().filter(|o| !first.iter().any(|f| f.kind == o.kind && f.name == o.name)) {
                match o.kind.as_str() {
                    "table" => sql.exec(&format!("DROP TABLE IF EXISTS {}", o.name), None)?,
                    "index" => sql.exec(&format!("DROP INDEX IF EXISTS {}", o.name), None)?,
                    other => return Err(CellError::invalid(format!("a later step's {other} {}: this lever drops tables and indexes", o.name))),
                };
            }
            // and the columns later steps added: step 5's `ended.search`
            sql.exec(&format!("ALTER TABLE ended DROP COLUMN search; DROP TABLE _schema; {plant}"), None)?;
            Ok(json!({ "members": columns(&sql, "members")?, "runs": columns(&sql, "runs")? }))
        }
        ("schema-try", Some(step @ ("adds" | "fails"))) => {
            let extra = match step {
                "adds" => Step { name: "e2e_probe", sql: "ALTER TABLE members ADD COLUMN e2e_probe TEXT;\n" },
                _ => Step { name: "e2e_fails", sql: "CREATE TABLE e2e_probe (a TEXT);\nINSERT INTO e2e_no_such_table VALUES (1);\n" },
            };
            // a test lever's own class, kept for the isolate's life: a few
            // bytes a pull, on fleets with test hooks alone
            let steps: &'static [Step] = Box::leak(class.steps.iter().copied().chain([extra]).collect::<Vec<_>>().into_boxed_slice());
            let ran = answer(migrated(raw, sql.clone(), class, steps, id));
            let probe: Vec<Value> = sql.exec("SELECT COUNT(*) AS n FROM sqlite_master WHERE name = 'e2e_probe'", None)?.to_array()?;
            Ok(json!({ "ran": ran["ran"], "refused": ran["refused"], "probeTable": probe.first().map(|r| r["n"].clone()) }))
        }
        _ => Err(CellError::invalid(format!("no schema lever {op} {shape:?}"))),
    }
}
