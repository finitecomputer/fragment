//! Feedback (docs/api.md, Feedback): what people send the deployment's
//! operators from the shell, kept here beside the operators' log. A
//! person sends at most `PER_DAY_MAX` a day, and the registry keeps at
//! most `KEPT_MAX` (past it a new one waits for an operator to delete
//! some). The router checked who asks of the operators' calls.
//!
//! Its screenshots' bytes are BLOBS', and a row here is their record until
//! they are gone (fragment_core::feedback::State), so none is ever left
//! unrecorded, whatever fails or ends between two steps:
//!
//! - `FeedbackBegin` keeps a send `sending` before the router puts a byte;
//!   `FeedbackKeep` makes it `kept` once all are. Both are idempotent on
//!   the send's id, its sender's: a send whose answer was lost is asked
//!   again, and is the same one.
//! - `FeedbackMailed` marks one kept whose operators' notice went, so a
//!   send asked again mails one that never did, and none twice.
//! - `FeedbackDelete` makes one `deleting`; so does this cell's alarm, of a
//!   send not kept in `SENDING_TTL_MS`. The alarm then deletes each
//!   deleting one's bytes, and only then its row (`feedback_alarm`); a run
//!   that fails arms the next.
//!
//! A wipe of the person deletes the bytes, then the rows (wipe.rs).

use fragment_core::feedback::{self as rules, State, DAY_MS, KEPT_MAX, PER_DAY_MAX, SENDING_TTL_MS, SWEEP_BATCH};
use fragment_proto::feedback::{Feedback, FeedbackKind, FeedbackPage, Screenshot};
use serde::de::IgnoredAny;
use serde::Serialize;

use super::calls::Call;
use super::*;

/// Feedback a page of the admin's lists: each holds up to 5,000 characters.
const PAGE: u32 = 50;
/// The next run of the alarm, when a run left more to delete.
const SWEEP_AGAIN_MS: i64 = 1000;
/// The next run of the alarm, after one that failed (its bytes' store did
/// not answer): it never ends the deleting.
const SWEEP_RETRY_MS: i64 = 60 * 1000;

/// `POST /feedback/begin`: a send, before its screenshots' bytes are put:
/// kept `sending` (a person, under the day's count, the registry not
/// full), or, asked again with its id, the same send: one still sending is
/// sent again as asked now, and one kept is answered as it was.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackBegin {
    pub by: By,
    pub id: String,
    pub kind: FeedbackKind,
    pub message: String,
    pub page: Option<String>,
    pub user_agent: Option<String>,
    pub release: String,
    pub screenshots: Vec<Screenshot>,
}

/// Whose it is; whether it was begun before (its earlier try's bytes may
/// be there); and, once kept, the feedback (nothing more to put, but its
/// notice, if it never went).
#[derive(Serialize, Deserialize)]
pub(crate) struct Began {
    pub person: String,
    pub again: bool,
    pub kept: Option<Kept>,
}

impl Call for FeedbackBegin {
    const PATH: &'static str = "/feedback/begin";
    type Answer = Began;
}

/// `POST /feedback/keep`: a send whose screenshots' bytes are all put,
/// kept. Asked again, it answers the one kept.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackKeep {
    pub by: By,
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Kept {
    pub feedback: Feedback,
    /// Whether the operators' notice of it went (`FeedbackMailed`): one
    /// that has not is the router's to send, whichever try learns it is kept.
    pub mailed: bool,
}

impl Call for FeedbackKeep {
    const PATH: &'static str = "/feedback/keep";
    type Answer = Kept;
}

/// `POST /feedback/mailed`: the operators' notice of one kept went.
/// Asked again, it changes nothing.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackMailed {
    pub id: String,
}

impl Call for FeedbackMailed {
    const PATH: &'static str = "/feedback/mailed";
    type Answer = Value;
}

/// `POST /feedback/list`: a page of those kept, newest first, before
/// `before`; only those not done when `open`.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackList {
    pub before: Option<u64>,
    pub open: bool,
}

impl Call for FeedbackList {
    const PATH: &'static str = "/feedback/list";
    type Answer = FeedbackPage;
}

/// `POST /feedback/get`: one kept, by its id.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackGet {
    pub id: String,
}

impl Call for FeedbackGet {
    const PATH: &'static str = "/feedback/get";
    type Answer = Feedback;
}

/// `POST /feedback/resolve`: an operator (`by`) marks one done, or open
/// again; logged.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackResolve {
    pub by: String,
    pub id: String,
    pub resolved: bool,
}

impl Call for FeedbackResolve {
    const PATH: &'static str = "/feedback/resolve";
    type Answer = Feedback;
}

/// `POST /feedback/delete`: an operator (`by`) deletes one, logged: it is
/// `deleting`, no longer listed or served, and the alarm, armed now,
/// deletes its bytes, then its row. Answers it as it was.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackDelete {
    pub by: String,
    pub id: String,
}

impl Call for FeedbackDelete {
    const PATH: &'static str = "/feedback/delete";
    type Answer = Feedback;
}

#[derive(Deserialize)]
struct Row {
    n: i64,
    id: String,
    person: String,
    email: Option<String>,
    kind: String,
    message: String,
    page: Option<String>,
    user_agent: Option<String>,
    release: String,
    screenshots: String,
    created_at: i64,
    resolved_at: Option<i64>,
    resolved_by: Option<String>,
    state: String,
    mailed_at: Option<i64>,
}

const SELECT: &str = "SELECT n, id, person, email, kind, message, page, user_agent, release, screenshots, created_at, resolved_at, resolved_by, state, mailed_at FROM feedback";

impl Row {
    /// One kept, as `FeedbackKeep` answers it.
    fn kept(self) -> CellResult<Kept> {
        let mailed = self.mailed_at.is_some();
        Ok(Kept { feedback: self.wire()?, mailed })
    }

    fn state(&self) -> CellResult<State> {
        State::parse(&self.state).ok_or_else(|| CellError::host(format!("feedback.state of {} is {:?}", self.id, self.state)))
    }

    fn wire(self) -> CellResult<Feedback> {
        let kind = FeedbackKind::parse(&self.kind).ok_or_else(|| CellError::host(format!("feedback.kind of {} is {:?}", self.id, self.kind)))?;
        let screenshots: Vec<Screenshot> = serde_json::from_str(&self.screenshots).map_err(|e| CellError::host(format!("feedback.screenshots of {}: {e}", self.id)))?;
        if screenshots.len() > rules::SCREENSHOTS_MAX {
            return Err(CellError::host(format!("feedback {} holds {} screenshots", self.id, screenshots.len())));
        }
        let n = u64::try_from(self.n).map_err(|_| CellError::host(format!("feedback.n of {} is {}", self.id, self.n)))?;
        Ok(Feedback {
            id: self.id,
            n,
            person: self.person,
            email: self.email,
            kind,
            message: self.message,
            page: self.page,
            user_agent: self.user_agent,
            release: self.release,
            screenshots,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolved_by: self.resolved_by,
        })
    }
}

/// One the alarm deletes: where its bytes are.
#[derive(Deserialize)]
struct Doomed {
    id: String,
    person: String,
}

/// When the alarm is next due for feedback: the oldest send's expiry, and
/// whether any is deleting.
#[derive(Deserialize)]
struct Due {
    sending: Option<i64>,
    deleting: i64,
}

impl RegistryCell {
    /// The person who asks, if they may send one more now.
    fn feedback_room(&self, by: &By) -> CellResult<Identity> {
        let who = self.by(by)?;
        if who.kind != IdentityKind::Person {
            return Err(CellError::new(ErrorCode::Forbidden, "feedback is a person's: an agent's owner sends it"));
        }
        // every send begun counts, kept or not: one never kept still put bytes
        let today = self.count("SELECT COUNT(*) AS n FROM feedback WHERE person = ? AND created_at > ?", vec![who.id.as_str().into(), SqlStorageValue::Integer(js::now_ms() - DAY_MS)])?;
        if today >= PER_DAY_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, format!("you have sent {PER_DAY_MAX} in a day, as many as we take: send more tomorrow")));
        }
        if self.count("SELECT COUNT(*) AS n FROM feedback", vec![])? >= KEPT_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, "we can't take more feedback right now: try again later"));
        }
        Ok(who)
    }

    /// One, whatever its state.
    fn feedback_row(&self, id: &str) -> CellResult<Option<Row>> {
        if !rules::valid_id(id) {
            return Err(CellError::new(ErrorCode::NotFound, format!("no feedback {id:?}")));
        }
        self.row::<Row>(&format!("{SELECT} WHERE id = ?"), vec![id.into()])
    }

    /// One kept: the operators see no other.
    fn feedback_of(&self, id: &str) -> CellResult<Feedback> {
        match self.feedback_row(id)? {
            Some(row) if row.state()? == State::Kept => row.wire(),
            _ => Err(CellError::new(ErrorCode::NotFound, format!("no feedback {id}"))),
        }
    }

    /// One of the asker's own, by its id (another's is refused, as a
    /// guessed id must not reach it); `None` when there is none.
    fn feedback_mine(&self, id: &str, who: &Identity) -> CellResult<Option<Row>> {
        if !rules::valid_id(id) {
            return Err(CellError::invalid("a feedback's id is 32 lowercase hex digits"));
        }
        match self.feedback_row(id)? {
            Some(row) if row.person != who.id => Err(CellError::new(ErrorCode::AlreadyExists, format!("feedback {id} is someone else's: send yours with an id of its own"))),
            row => Ok(row),
        }
    }

    async fn feedback_begin(&self, b: FeedbackBegin) -> CellResult<Began> {
        // the router read the form (fragment_core::feedback::read); checked
        // again here, as what is stored is this cell's to keep sound
        let chars = b.message.chars().count();
        assert!(chars > 0 && chars <= rules::MESSAGE_MAX_CHARS, "the router sends a message read");
        assert!(b.page.as_deref().is_none_or(rules::valid_page), "the router sends a page read");
        assert!(b.screenshots.len() <= rules::SCREENSHOTS_MAX, "the router sends a few screenshots");
        // armed before its row is written: a send never kept is never left
        self.arm_feedback(js::now_ms() + SENDING_TTL_MS).await?;
        let who = self.by(&b.by)?;
        let shots = serde_json::to_string(&b.screenshots).map_err(|e| CellError::host(e.to_string()))?;
        let null = |v: Option<String>| v.map_or(SqlStorageValue::Null, Into::into);
        match self.feedback_mine(&b.id, &who)? {
            Some(row) => match row.state()? {
                State::Kept => Ok(Began { person: who.id, again: true, kept: Some(row.kept()?) }),
                // the same send, asked again: as it is asked now
                State::Sending => {
                    self.exec(
                        "UPDATE feedback SET kind = ?, message = ?, page = ?, user_agent = ?, release = ?, screenshots = ? WHERE id = ? AND state = 'sending'",
                        vec![b.kind.as_str().into(), b.message.into(), null(b.page), null(b.user_agent), b.release.into(), shots.into(), b.id.as_str().into()],
                    )?;
                    Ok(Began { person: who.id, again: true, kept: None })
                }
                State::Deleting => Err(CellError::new(ErrorCode::AlreadyExists, format!("feedback {} was deleted: send it again, as a new one", b.id))),
            },
            None => {
                let who = self.feedback_room(&b.by)?;
                self.exec(
                    "INSERT INTO feedback (id, person, email, kind, message, page, user_agent, release, screenshots, created_at, state) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'sending')",
                    vec![
                        b.id.as_str().into(),
                        who.id.as_str().into(),
                        null(self.email_of(&who.id)?),
                        b.kind.as_str().into(),
                        b.message.into(),
                        null(b.page),
                        null(b.user_agent),
                        b.release.into(),
                        shots.into(),
                        SqlStorageValue::Integer(js::now_ms()),
                    ],
                )?;
                Ok(Began { person: who.id, again: false, kept: None })
            }
        }
    }

    fn feedback_keep(&self, b: FeedbackKeep) -> CellResult<Kept> {
        let who = self.by(&b.by)?;
        let row = self.feedback_mine(&b.id, &who)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no feedback {} being sent: it took too long, send it again", b.id)))?;
        match row.state()? {
            State::Kept => row.kept(),
            State::Sending => {
                self.exec("UPDATE feedback SET state = 'kept' WHERE id = ? AND state = 'sending'", vec![b.id.as_str().into()])?;
                self.feedback_row(&b.id)?.ok_or_else(|| CellError::host(format!("feedback {} was kept, and is gone", b.id)))?.kept()
            }
            State::Deleting => Err(CellError::new(ErrorCode::NotFound, format!("no feedback {} being sent: it took too long, send it again", b.id))),
        }
    }

    fn feedback_mailed(&self, id: &str) -> CellResult<Value> {
        let marked = self.rows::<IgnoredAny>("UPDATE feedback SET mailed_at = ? WHERE id = ? AND state = 'kept' AND mailed_at IS NULL RETURNING id", vec![SqlStorageValue::Integer(js::now_ms()), id.into()])?;
        Ok(json!({ "marked": !marked.is_empty() }))
    }

    fn feedback_list(&self, b: FeedbackList) -> CellResult<FeedbackPage> {
        let before = b.before.map_or(i64::MAX, |n| i64::try_from(n).unwrap_or(i64::MAX));
        let open = if b.open { " AND resolved_at IS NULL" } else { "" };
        let rows = self.rows::<Row>(&format!("{SELECT} WHERE n < ? AND state = 'kept'{open} ORDER BY n DESC LIMIT ?"), vec![SqlStorageValue::Integer(before), SqlStorageValue::Integer(PAGE.into())])?;
        assert!(rows.len() <= PAGE as usize, "a page is bounded");
        let next = (rows.len() == PAGE as usize).then(|| rows.last().map(|r| r.n as u64)).flatten();
        Ok(FeedbackPage { feedback: rows.into_iter().map(Row::wire).collect::<CellResult<_>>()?, next })
    }

    fn feedback_resolve(&self, b: FeedbackResolve) -> CellResult<Feedback> {
        let was = self.feedback_of(&b.id)?;
        // asked again, it changes nothing and logs nothing
        if was.resolved_at.is_some() == b.resolved {
            return Ok(was);
        }
        match b.resolved {
            true => self.exec("UPDATE feedback SET resolved_at = ?, resolved_by = ? WHERE id = ?", vec![SqlStorageValue::Integer(js::now_ms()), b.by.as_str().into(), b.id.as_str().into()])?,
            false => self.exec("UPDATE feedback SET resolved_at = NULL, resolved_by = NULL WHERE id = ?", vec![b.id.as_str().into()])?,
        }
        self.log_admin(&b.by, if b.resolved { "feedback-done" } else { "feedback-reopen" }, &b.id, &format!("{} from {}", was.kind.as_str(), was.email.as_deref().unwrap_or(&was.person)))?;
        self.feedback_of(&b.id)
    }

    async fn feedback_delete(&self, b: FeedbackDelete) -> CellResult<Feedback> {
        let was = self.feedback_of(&b.id)?;
        // armed before its row changes: a deleting one is never left
        self.arm_feedback(js::now_ms()).await?;
        // between the read and here another delete may have come: one log
        let changed = self.rows::<IgnoredAny>("UPDATE feedback SET state = 'deleting' WHERE id = ? AND state = 'kept' RETURNING id", vec![b.id.as_str().into()])?;
        if changed.is_empty() {
            return Err(CellError::new(ErrorCode::NotFound, format!("no feedback {}", b.id)));
        }
        self.log_admin(&b.by, "feedback-delete", &b.id, &format!("{} from {}", was.kind.as_str(), was.email.as_deref().unwrap_or(&was.person)))?;
        Ok(was)
    }

    /// Arms the alarm by `at`, unless it is armed sooner already.
    async fn arm_feedback(&self, at: i64) -> CellResult<()> {
        let storage = self.state.storage();
        match storage.get_alarm().await? {
            Some(armed) if armed <= at => Ok(()),
            _ => Ok(storage.set_alarm(super::signin::alarm_at(at)).await?),
        }
    }

    /// When the alarm is next due for feedback: now when one is deleting,
    /// else when the oldest send expires (`None`: nothing to do).
    fn feedback_due(&self, now: i64) -> CellResult<Option<i64>> {
        let d = self
            .row::<Due>("SELECT (SELECT MIN(created_at) FROM feedback WHERE state = 'sending') AS sending, (SELECT COUNT(*) FROM feedback WHERE state = 'deleting') AS deleting", vec![])?
            .ok_or_else(|| CellError::host("the feedback's MIN answered no row"))?;
        Ok(if d.deleting > 0 { Some(now + SWEEP_AGAIN_MS) } else { d.sending.map(|at| (at + SENDING_TTL_MS).max(now + SWEEP_AGAIN_MS)) })
    }

    /// The registry's alarm, for feedback: a send not kept in time is
    /// deleting; then a batch of deleting ones, each its bytes deleted,
    /// then its row; then the next run armed.
    pub(super) async fn feedback_alarm(&self) -> CellResult<()> {
        let fails = self.feedback_fails.get();
        if fails > 0 {
            self.feedback_fails.set(fails - 1);
            return Err(CellError::host("the feedback's bytes did not delete (a test hook)"));
        }
        let now = js::now_ms();
        self.exec("UPDATE feedback SET state = 'deleting' WHERE state = 'sending' AND created_at <= ?", vec![SqlStorageValue::Integer(now - SENDING_TTL_MS)])?;
        let doomed = self.rows::<Doomed>("SELECT id, person FROM feedback WHERE state = 'deleting' ORDER BY n LIMIT ?", vec![SqlStorageValue::Integer(SWEEP_BATCH.into())])?;
        assert!(doomed.len() <= SWEEP_BATCH as usize, "a run deletes at most a batch");
        for d in &doomed {
            // every place under it, whatever a try of the send put
            let (_, more) = js::blob_delete_under(&self.env, &rules::feedback_prefix(&d.person, &d.id), 1).await?;
            assert!(!more, "a feedback's screenshots are a page's few");
            self.exec("DELETE FROM feedback WHERE id = ? AND state = 'deleting'", vec![d.id.as_str().into()])?;
        }
        match self.feedback_due(js::now_ms())? {
            Some(at) => self.arm_feedback(at).await,
            None => Ok(()),
        }
    }

    /// The test hook's `FeedbackAge`: a send still sending made as old as
    /// one the alarm deletes, and the alarm armed now. Answers how many.
    pub(super) async fn feedback_age(&self, id: &str) -> CellResult<usize> {
        if !rules::valid_id(id) {
            return Err(CellError::invalid("a feedback's id is 32 lowercase hex digits"));
        }
        let aged = self.rows::<IgnoredAny>("UPDATE feedback SET created_at = created_at - ? WHERE id = ? AND state = 'sending' RETURNING id", vec![SqlStorageValue::Integer(SENDING_TTL_MS), id.into()])?;
        self.arm_feedback(js::now_ms()).await?;
        Ok(aged.len())
    }

    /// After a failed run: the next one, soon, so a failure never ends it.
    pub(super) async fn feedback_later(&self) -> CellResult<()> {
        self.arm_feedback(js::now_ms() + SWEEP_RETRY_MS).await
    }

    pub(super) async fn feedback_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            FeedbackBegin::PATH => reply::<FeedbackBegin>(match body(bytes) {
                Ok(b) => self.feedback_begin(b).await,
                Err(e) => Err(e),
            }),
            FeedbackKeep::PATH => reply::<FeedbackKeep>(body(bytes).and_then(|b| self.feedback_keep(b))),
            FeedbackMailed::PATH => reply::<FeedbackMailed>(body::<FeedbackMailed>(bytes).and_then(|b| self.feedback_mailed(&b.id))),
            FeedbackList::PATH => reply::<FeedbackList>(body(bytes).and_then(|b| self.feedback_list(b))),
            FeedbackGet::PATH => reply::<FeedbackGet>(body::<FeedbackGet>(bytes).and_then(|b| self.feedback_of(&b.id))),
            FeedbackResolve::PATH => reply::<FeedbackResolve>(body(bytes).and_then(|b| self.feedback_resolve(b))),
            FeedbackDelete::PATH => reply::<FeedbackDelete>(match body(bytes) {
                Ok(b) => self.feedback_delete(b).await,
                Err(e) => Err(e),
            }),
            _ => return None,
        })
    }
}
