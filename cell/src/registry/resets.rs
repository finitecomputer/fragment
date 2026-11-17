//! An operator's reset of this month's usage (docs/ledger.md, "A month's
//! usage reset"), on every person's and org's ledger, or on one payer's:
//! a run the registry keeps (`usage_resets`), with a row for each payer it
//! resets (`usage_reset_payers`), all queued in the turn that starts it.
//! The registry's alarm resets a batch of due payers at a time
//! (`ledger::reset::BATCH`), each one call to its ledger,
//! `ResetMonthUsage {id: "reset:<run>"}`, and arms itself again while any
//! is due, as it pushes seat holders' plans (orgs.rs).
//!
//! A run resumes wherever it stopped: a payer's row is done only once its
//! ledger answered, and a ledger answers the same reset again as it did
//! the first time (`Ledger::reset_month`), so a payer cut off mid-call is
//! asked again and counted once. A failed call is tried again later,
//! `ledger::reset::TRIES` times in all, then given up on with its last
//! refusal. The same run's id again answers the run; one run goes at a
//! time. The operators' log holds who started each run and when, and its
//! counts once it is done.
//!
//! Only an operator starts one: the router checked who asks, and the
//! registry checks the operator again here (`operator_named`).

use fragment_core::ledger::org::Payer;
use fragment_core::ledger::reset::{retry_after_ms, valid_run_id, BATCH, FAILURES_SHOWN, RUNS_KEPT, RUN_ID_MAX_BYTES, TRIES};
use fragment_core::ledger::Refused;
use fragment_proto::ledger::ResetMonthUsage;
use fragment_proto::org::{StartUsageReset, UsageReset, UsageResetFailure, UsageResets};
use serde::Serialize;

use super::calls::Call;
use super::*;

/// `POST /admin/usage-resets/start`: an operator (`by`, the router's
/// name for them) starts a run, or asks again for one they started.
#[derive(Serialize, Deserialize)]
pub(crate) struct StartReset {
    pub by: String,
    pub start: StartUsageReset,
}

impl Call for StartReset {
    const PATH: &'static str = "/admin/usage-resets/start";
    type Answer = UsageReset;
}

/// `POST /admin/usage-resets/of`: one run, as far as it has gone.
#[derive(Serialize, Deserialize)]
pub(crate) struct ResetOf {
    pub id: String,
}

impl Call for ResetOf {
    const PATH: &'static str = "/admin/usage-resets/of";
    type Answer = UsageReset;
}

/// `POST /admin/usage-resets/list`: the runs kept, newest first.
#[derive(Serialize, Deserialize)]
pub(crate) struct ResetList {}

impl Call for ResetList {
    const PATH: &'static str = "/admin/usage-resets/list";
    type Answer = UsageResets;
}

/// A run's row and its payers' counts, in one statement.
#[derive(Deserialize)]
struct RunRow {
    id: String,
    operator: String,
    payer: Option<String>,
    started_at: i64,
    finished_at: Option<i64>,
    people: u64,
    orgs: u64,
    reset: u64,
    restored: i64,
    pending: u64,
    retrying: u64,
    failed: u64,
}

const RUN_SELECT: &str = "SELECT r.id, r.operator, r.payer, r.started_at, r.finished_at,
  (SELECT COUNT(*) FROM usage_reset_payers WHERE run = r.id AND substr(payer, 1, 4) != 'org:') AS people,
  (SELECT COUNT(*) FROM usage_reset_payers WHERE run = r.id AND substr(payer, 1, 4) = 'org:') AS orgs,
  (SELECT COUNT(*) FROM usage_reset_payers WHERE run = r.id AND restored IS NOT NULL) AS reset,
  (SELECT COALESCE(SUM(restored), 0) FROM usage_reset_payers WHERE run = r.id) AS restored,
  (SELECT COUNT(*) FROM usage_reset_payers WHERE run = r.id AND done_at IS NULL) AS pending,
  (SELECT COUNT(*) FROM usage_reset_payers WHERE run = r.id AND done_at IS NULL AND tries > 0) AS retrying,
  (SELECT COUNT(*) FROM usage_reset_payers WHERE run = r.id AND done_at IS NOT NULL AND restored IS NULL) AS failed
  FROM usage_resets r";

/// A due payer of a run.
#[derive(Deserialize)]
struct DueRow {
    run: String,
    payer: String,
    tries: i64,
}

/// What the operators' log says of a run's payers.
fn whom(payer: Option<&str>) -> &str {
    payer.unwrap_or("everyone")
}

impl RegistryCell {
    /// Whether `by`, as the router names an operator (`orgs::operator`),
    /// is one (`fragment_core::registry::names_operator`): an identity the
    /// deployment lists, a key it lists that no one holds (by its npub), or
    /// a person who holds a key it lists, not revoked.
    fn operator_named(&self, by: &str) -> CellResult<bool> {
        #[derive(Deserialize)]
        struct Key {
            key: String,
        }
        // bounded: an identity holds at most KEYS_PER_IDENTITY_MAX keys
        let held: Vec<String> = self.rows::<Key>("SELECT key FROM keys WHERE identity = ? AND revoked_at IS NULL", vec![by.into()])?.into_iter().map(|k| k.key).collect();
        Ok(fragment_core::registry::names_operator(self.cfg.operators()?, by, &held))
    }

    fn run_of(&self, id: &str) -> CellResult<Option<UsageReset>> {
        let Some(r) = self.row::<RunRow>(&format!("{RUN_SELECT} WHERE r.id = ?"), vec![id.into()])? else { return Ok(None) };
        let failed = self.rows::<UsageResetFailure>(
            "SELECT payer, COALESCE(why, '') AS why FROM usage_reset_payers WHERE run = ? AND done_at IS NOT NULL AND restored IS NULL ORDER BY payer LIMIT ?",
            vec![id.into(), SqlStorageValue::Integer(FAILURES_SHOWN)],
        )?;
        Ok(Some(UsageReset {
            id: r.id,
            operator: r.operator,
            payer: r.payer,
            started_at: r.started_at,
            finished_at: r.finished_at,
            people: r.people,
            orgs: r.orgs,
            reset: r.reset,
            restored_micros: r.restored,
            pending: r.pending,
            retrying: r.retrying,
            failed,
            failed_count: r.failed,
        }))
    }

    fn named_run(&self, id: &str) -> CellResult<UsageReset> {
        self.run_of(id)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no usage reset {id}")))
    }

    /// Starts a run, its payers queued in this turn, or answers the run of
    /// this id again (another payer under it is refused). One run at a
    /// time: another, still going, is refused (409) by its id.
    fn start_reset(&self, b: StartReset) -> CellResult<UsageReset> {
        if !self.operator_named(&b.by)? {
            return Err(CellError::new(ErrorCode::Forbidden, "only the deployment's operators reset usage"));
        }
        let StartUsageReset { id, payer } = b.start;
        if !valid_run_id(&id) {
            return Err(CellError::invalid(format!("a reset's id is 1 to {RUN_ID_MAX_BYTES} of A-Z, a-z, 0-9, -, _, . and :")));
        }
        if let Some(run) = self.run_of(&id)? {
            return match run.payer == payer {
                true => Ok(run),
                false => Err(CellError::new(ErrorCode::ConflictingBody, format!("the usage reset {id} was of {}", whom(run.payer.as_deref())))),
            };
        }
        #[derive(Deserialize)]
        struct Running {
            id: String,
        }
        if let Some(running) = self.row::<Running>("SELECT id FROM usage_resets WHERE finished_at IS NULL LIMIT 1", vec![])? {
            return Err(CellError::new(ErrorCode::Conflict, format!("the usage reset {} is still going: one runs at a time", running.id)));
        }
        if let Some(p) = &payer {
            let there = match Payer::parse(p) {
                Some(Payer::Person(person)) => self.count("SELECT COUNT(*) AS n FROM identities WHERE id = ? AND kind = 'person'", vec![person.into()])? > 0,
                Some(Payer::Org(org)) => self.org_row(&org)?.is_some(),
                None => return Err(CellError::invalid(format!("{p:?} names no payer: a person's identity (an npub), or org:<org id>"))),
            };
            if !there {
                return Err(CellError::new(ErrorCode::NotFound, format!("no payer {p}")));
            }
        }
        let now = SqlStorageValue::Integer(js::now_ms());
        // the oldest runs past those kept go, this one counted
        let kept = SqlStorageValue::Integer(RUNS_KEPT - 1);
        self.exec("DELETE FROM usage_reset_payers WHERE run IN (SELECT id FROM usage_resets ORDER BY started_at DESC, id LIMIT -1 OFFSET ?)", vec![kept.clone()])?;
        self.exec("DELETE FROM usage_resets WHERE id IN (SELECT id FROM usage_resets ORDER BY started_at DESC, id LIMIT -1 OFFSET ?)", vec![kept])?;
        self.exec(
            "INSERT INTO usage_resets (id, operator, payer, started_at) VALUES (?, ?, ?, ?)",
            vec![id.as_str().into(), b.by.as_str().into(), payer.as_deref().map_or(SqlStorageValue::Null, Into::into), now.clone()],
        )?;
        let run = SqlStorageValue::from(id.as_str());
        match &payer {
            Some(p) => self.exec("INSERT INTO usage_reset_payers (run, payer, due, tries) VALUES (?, ?, ?, 0)", vec![run, p.as_str().into(), now])?,
            // everyone there now: a person or an org made after it starts
            // has used nothing of the month it resets
            None => {
                self.exec("INSERT INTO usage_reset_payers (run, payer, due, tries) SELECT ?, id, ?, 0 FROM identities WHERE kind = 'person'", vec![run.clone(), now.clone()])?;
                self.exec("INSERT INTO usage_reset_payers (run, payer, due, tries) SELECT ?, 'org:' || id, ?, 0 FROM orgs", vec![run, now])?;
            }
        }
        let started = self.named_run(&id)?;
        self.log_admin(&b.by, "usage-reset", whom(payer.as_deref()), &format!("{id} started: {} people, {} orgs", started.people, started.orgs))?;
        self.finish_resets()?;
        self.named_run(&id)
    }

    /// Each run whose payers are all done: finished now, its counts in
    /// the operators' log.
    fn finish_resets(&self) -> CellResult<()> {
        #[derive(Deserialize)]
        struct Done {
            id: String,
        }
        let done = self.rows::<Done>(
            "SELECT id FROM usage_resets r WHERE finished_at IS NULL AND NOT EXISTS (SELECT 1 FROM usage_reset_payers WHERE run = r.id AND done_at IS NULL)",
            vec![],
        )?;
        // bounded: one run goes at a time
        for d in done {
            self.exec("UPDATE usage_resets SET finished_at = ? WHERE id = ?", vec![SqlStorageValue::Integer(js::now_ms()), d.id.as_str().into()])?;
            let r = self.named_run(&d.id)?;
            let detail = format!(
                "{} done: {} of {} reset ({} people, {} orgs), {} restored, {} failed",
                r.id,
                r.reset,
                r.people + r.orgs,
                r.people,
                r.orgs,
                fragment_core::price::dollars(r.restored_micros),
                r.failed_count
            );
            self.log_admin(&r.operator, "usage-reset", whom(r.payer.as_deref()), &detail)?;
        }
        Ok(())
    }

    /// The alarm's resets: a batch of the due payers, each asked of its
    /// ledger in turn and its row written as it is answered; then each run
    /// done is finished, and the alarm armed for the next due payer.
    pub(super) async fn resets_alarm(&self) -> CellResult<()> {
        let due = self.rows::<DueRow>(
            "SELECT run, payer, tries FROM usage_reset_payers WHERE done_at IS NULL AND due <= ? ORDER BY due, run, payer LIMIT ?",
            vec![SqlStorageValue::Integer(js::now_ms()), SqlStorageValue::Integer(BATCH.into())],
        )?;
        // bounded: at most BATCH ledgers a run
        for d in due {
            let key = vec![d.run.as_str().into(), d.payer.as_str().into()];
            let asked = crate::ledger::ask(&self.env, &d.payer, &ResetMonthUsage { id: format!("reset:{}", d.run) }).await;
            let now = SqlStorageValue::Integer(js::now_ms());
            match asked {
                Ok(r) => self.exec("UPDATE usage_reset_payers SET done_at = ?, restored = ?, why = NULL WHERE run = ? AND payer = ?", [vec![now, SqlStorageValue::Integer(r.restored_micros)], key].concat())?,
                // a person wiped since the run started has nothing to reset
                Err(e) if e.refused == Some(Refused::Wiped) => self.exec("UPDATE usage_reset_payers SET done_at = ?, restored = 0, why = NULL WHERE run = ? AND payer = ?", [vec![now], key].concat())?,
                Err(e) if d.tries + 1 >= TRIES => {
                    console_error!("{}", json!({ "usage-reset": d.run, "payer": d.payer, "given-up": e.message }));
                    self.exec("UPDATE usage_reset_payers SET done_at = ?, tries = tries + 1, why = ? WHERE run = ? AND payer = ?", [vec![now, e.message.as_str().into()], key].concat())?;
                }
                Err(e) => {
                    console_error!("{}", json!({ "usage-reset": d.run, "payer": d.payer, "tries": d.tries + 1, "failed": e.message }));
                    let due = SqlStorageValue::Integer(js::now_ms() + retry_after_ms(d.tries));
                    self.exec("UPDATE usage_reset_payers SET due = ?, tries = tries + 1, why = ? WHERE run = ? AND payer = ?", [vec![due, e.message.as_str().into()], key].concat())?;
                }
            }
        }
        self.finish_resets()?;
        self.arm_resets().await
    }

    /// Arms the alarm for the next due payer, unless it is armed sooner.
    async fn arm_resets(&self) -> CellResult<()> {
        #[derive(Deserialize)]
        struct Next {
            due: Option<i64>,
        }
        match self.row::<Next>("SELECT MIN(due) AS due FROM usage_reset_payers WHERE done_at IS NULL", vec![])?.and_then(|r| r.due) {
            Some(due) => self.arm_at(due.max(js::now_ms())).await,
            None => Ok(()),
        }
    }

    /// The registry's reset routes, or `None` for a path that is not one.
    pub(super) async fn resets_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            StartReset::PATH => {
                let started = body::<StartReset>(bytes).and_then(|b| self.start_reset(b));
                if started.is_ok() {
                    if let Err(e) = self.arm_resets().await {
                        // its payers are queued: the alarm's next run takes them
                        console_error!("the registry's usage reset was not armed ({:?}): {}", e.code, e.message);
                    }
                }
                reply::<StartReset>(started)
            }
            ResetOf::PATH => reply::<ResetOf>(body::<ResetOf>(bytes).and_then(|b| self.named_run(&b.id))),
            ResetList::PATH => reply::<ResetList>(body::<ResetList>(bytes).and_then(|ResetList {}| {
                #[derive(Deserialize)]
                struct Id {
                    id: String,
                }
                let ids = self.rows::<Id>("SELECT id FROM usage_resets ORDER BY started_at DESC, id LIMIT ?", vec![SqlStorageValue::Integer(RUNS_KEPT)])?;
                Ok(UsageResets { runs: ids.iter().map(|i| self.named_run(&i.id)).collect::<CellResult<_>>()? })
            })),
            _ => return None,
        })
    }
}
