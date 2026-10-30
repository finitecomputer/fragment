//! Feedback (docs/api.md, Feedback): what people send the deployment's
//! operators from the shell, kept here beside the operators' log. A
//! person sends at most `PER_DAY_MAX` a day, and the registry keeps at
//! most `KEPT_MAX` (past it a new one waits for an operator to delete
//! some). The router checked who asks of the operators' calls.
//!
//! Its screenshots' bytes are BLOBS', and a row here is their record until
//! they are gone (fragment_core::feedback::State), so none is ever left
//! unrecorded, whatever fails, ends, or runs at once:
//!
//! - `FeedbackBegin` keeps a send `sending` before the router puts a byte,
//!   and makes it a try of its own (`attempt`) each time it is asked: the
//!   router puts that try's bytes under its own keys, and only the latest
//!   try may `FeedbackKeep` it, so two tries in flight at once never mix,
//!   and a stale one cannot touch a kept one's bytes. Both are idempotent
//!   on the send's id, its sender's: a send whose answer was lost is asked
//!   again, and is the same one.
//! - A try starts no put later than `WRITE_WINDOW_MS` after it asked to
//!   begin, so none of its bytes lands past `FENCE_MS` from its beginning:
//!   a row's `fenced_until`. Nothing under a row is deleted before it.
//! - `FeedbackMailed` marks one kept whose operators' notice went, so a
//!   send asked again mails one that never did, and none twice.
//! - `FeedbackDelete` makes one `deleting`; so does this cell's alarm, of a
//!   send not kept in `SENDING_TTL_MS` from its latest try. The alarm then
//!   deletes each deleting one's bytes once past its fence, and only then
//!   its row; and, past a kept one's fence, its earlier tries' bytes
//!   (`feedback_alarm`). A run that fails arms the next.
//!
//! A wipe of the person waits out their fences, deletes the bytes, then
//! the rows (wipe.rs).

use fragment_core::feedback::{self as rules, State, ATTEMPTS_MAX, DAY_MS, FENCE_MS, KEPT_MAX, PER_DAY_MAX, SENDING_TTL_MS, SWEEP_BATCH};
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

/// `POST /feedback/begin`: a try of a send, before its screenshots' bytes
/// are put: a new send kept `sending` (a person, under the day's count,
/// the registry not full), or, asked again with its id, the same send:
/// one still sending is a new try, as asked now (the try before can keep
/// it no longer), and one kept is answered as it was.
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

/// Whose it is, and either the try to put its bytes as and keep it by,
/// or, kept already, the feedback (nothing more to put, but its notice, if
/// it never went).
#[derive(Serialize, Deserialize)]
pub(crate) struct Began {
    pub person: String,
    pub attempt: Option<String>,
    pub kept: Option<Kept>,
}

impl Call for FeedbackBegin {
    const PATH: &'static str = "/feedback/begin";
    type Answer = Began;
}

/// `POST /feedback/keep`: a try whose screenshots' bytes are all put
/// keeps its send, if it is the send's latest. Asked again, or once
/// another try kept it, it answers the one kept.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackKeep {
    pub by: By,
    pub id: String,
    pub attempt: String,
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

/// `POST /feedback/get`: one kept, by its id, and the try that kept it
/// (whose keys its screenshots are under).
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackGet {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Got {
    pub feedback: Feedback,
    pub attempt: String,
}

impl Call for FeedbackGet {
    const PATH: &'static str = "/feedback/get";
    type Answer = Got;
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
    attempt: String,
    attempts: i64,
    fenced_until: Option<i64>,
    mailed_at: Option<i64>,
}

const SELECT: &str = "SELECT n, id, person, email, kind, message, page, user_agent, release, screenshots, created_at, resolved_at, resolved_by, state, attempt, attempts, fenced_until, mailed_at FROM feedback";

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

/// One the alarm deletes, or prunes: where its bytes are, the try that
/// kept it, and its fence as read.
#[derive(Deserialize)]
struct Doomed {
    id: String,
    person: String,
    attempt: String,
    fenced_until: Option<i64>,
}

/// When the alarm is next due for feedback: the oldest send's latest try,
/// the earliest fence of one deleting (0 for none), and of one kept whose
/// earlier tries' bytes it prunes.
#[derive(Deserialize)]
struct Due {
    sending: Option<i64>,
    deleting: Option<i64>,
    pruning: Option<i64>,
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
        let now = js::now_ms();
        let who = self.by(&b.by)?;
        let attempt = js::random_hex::<8>();
        assert!(rules::valid_attempt(&attempt), "a try is 16 hex digits");
        let shots = serde_json::to_string(&b.screenshots).map_err(|e| CellError::host(e.to_string()))?;
        let null = |v: Option<String>| v.map_or(SqlStorageValue::Null, Into::into);
        match self.feedback_mine(&b.id, &who)? {
            Some(row) => match row.state()? {
                State::Kept => Ok(Began { person: who.id, attempt: None, kept: Some(row.kept()?) }),
                // the same send, asked again: a new try, as it is asked now;
                // its expiry and its fence are this try's
                State::Sending => {
                    if row.attempts >= i64::from(ATTEMPTS_MAX) {
                        return Err(CellError::new(ErrorCode::AlreadyExists, format!("feedback {} was tried {ATTEMPTS_MAX} times: send it again, as a new one", b.id)));
                    }
                    self.exec(
                        "UPDATE feedback SET kind = ?, message = ?, page = ?, user_agent = ?, release = ?, screenshots = ?, attempt = ?, attempt_at = ?, attempts = attempts + 1, fenced_until = ? WHERE id = ? AND state = 'sending'",
                        vec![
                            b.kind.as_str().into(),
                            b.message.into(),
                            null(b.page),
                            null(b.user_agent),
                            b.release.into(),
                            shots.into(),
                            attempt.as_str().into(),
                            SqlStorageValue::Integer(now),
                            SqlStorageValue::Integer(now + FENCE_MS),
                            b.id.as_str().into(),
                        ],
                    )?;
                    Ok(Began { person: who.id, attempt: Some(attempt), kept: None })
                }
                State::Deleting => Err(CellError::new(ErrorCode::AlreadyExists, format!("feedback {} was deleted: send it again, as a new one", b.id))),
            },
            None => {
                let who = self.feedback_room(&b.by)?;
                self.exec(
                    "INSERT INTO feedback (id, person, email, kind, message, page, user_agent, release, screenshots, created_at, state, attempt, attempt_at, attempts, fenced_until) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'sending', ?, ?, 1, ?)",
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
                        SqlStorageValue::Integer(now),
                        attempt.as_str().into(),
                        SqlStorageValue::Integer(now),
                        SqlStorageValue::Integer(now + FENCE_MS),
                    ],
                )?;
                Ok(Began { person: who.id, attempt: Some(attempt), kept: None })
            }
        }
    }

    async fn feedback_keep(&self, b: FeedbackKeep) -> CellResult<Kept> {
        let who = self.by(&b.by)?;
        let gone = || CellError::new(ErrorCode::NotFound, format!("no feedback {} being sent: it took too long, send it again", b.id));
        let row = self.feedback_mine(&b.id, &who)?.ok_or_else(gone)?;
        match row.state()? {
            // whichever try kept it: the send is kept
            State::Kept => row.kept(),
            State::Sending if row.attempt != b.attempt => Err(CellError::new(ErrorCode::AlreadyExists, format!("a later try of feedback {} is under way: it is that one's to keep", b.id))),
            State::Sending => {
                // its own bytes are all put: with no other try, none can land
                // later; else an earlier try's may, until the fence, when the
                // alarm prunes them
                if row.attempts > 1 {
                    if let Some(fence) = row.fenced_until {
                        self.arm_feedback(fence).await?;
                    }
                }
                let kept = self.rows::<IgnoredAny>(
                    "UPDATE feedback SET state = 'kept', fenced_until = CASE WHEN attempts = 1 THEN NULL ELSE fenced_until END WHERE id = ? AND state = 'sending' AND attempt = ? RETURNING id",
                    vec![b.id.as_str().into(), b.attempt.as_str().into()],
                )?;
                if kept.is_empty() {
                    // another try came while the alarm was armed
                    return Err(CellError::new(ErrorCode::AlreadyExists, format!("a later try of feedback {} is under way: it is that one's to keep", b.id)));
                }
                self.feedback_row(&b.id)?.ok_or_else(|| CellError::host(format!("feedback {} was kept, and is gone", b.id)))?.kept()
            }
            State::Deleting => Err(gone()),
        }
    }

    fn feedback_mailed(&self, id: &str) -> CellResult<Value> {
        let marked = self.rows::<IgnoredAny>("UPDATE feedback SET mailed_at = ? WHERE id = ? AND state = 'kept' AND mailed_at IS NULL RETURNING id", vec![SqlStorageValue::Integer(js::now_ms()), id.into()])?;
        Ok(json!({ "marked": !marked.is_empty() }))
    }

    fn feedback_got(&self, id: &str) -> CellResult<Got> {
        match self.feedback_row(id)? {
            Some(row) if row.state()? == State::Kept => {
                let attempt = row.attempt.clone();
                Ok(Got { feedback: row.wire()?, attempt })
            }
            _ => Err(CellError::new(ErrorCode::NotFound, format!("no feedback {id}"))),
        }
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
        let row = match self.feedback_row(&b.id)? {
            Some(row) if row.state()? == State::Kept => row,
            _ => return Err(CellError::new(ErrorCode::NotFound, format!("no feedback {}", b.id))),
        };
        // armed before its row changes, by its fence: a deleting one is never left
        self.arm_feedback(row.fenced_until.unwrap_or(0).max(js::now_ms())).await?;
        let was = row.wire()?;
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

    /// When the alarm is next due for feedback: when the oldest send
    /// expires, one deleting is past its fence, or one kept is past the
    /// fence of its earlier tries (`None`: nothing to do).
    fn feedback_due(&self, now: i64) -> CellResult<Option<i64>> {
        let d = self
            .row::<Due>(
                "SELECT (SELECT MIN(attempt_at) FROM feedback WHERE state = 'sending') AS sending,
                        (SELECT MIN(COALESCE(fenced_until, 0)) FROM feedback WHERE state = 'deleting') AS deleting,
                        (SELECT MIN(fenced_until) FROM feedback WHERE state = 'kept' AND fenced_until IS NOT NULL) AS pruning",
                vec![],
            )?
            .ok_or_else(|| CellError::host("the feedback's MIN answered no row"))?;
        let due = [d.sending.map(|at| at + SENDING_TTL_MS), d.deleting, d.pruning].into_iter().flatten().min();
        Ok(due.map(|at| at.max(now + SWEEP_AGAIN_MS)))
    }

    /// The registry's alarm, for feedback: a send not kept in time from its
    /// latest try is deleting; then a batch of deleting ones past their
    /// fence, each its bytes deleted, then its row; then a batch of kept
    /// ones past theirs, each its earlier tries' bytes deleted; then the
    /// next run armed.
    pub(super) async fn feedback_alarm(&self) -> CellResult<()> {
        let fails = self.feedback_fails.get();
        if fails > 0 {
            self.feedback_fails.set(fails - 1);
            return Err(CellError::host("the feedback's bytes did not delete (a test hook)"));
        }
        let now = js::now_ms();
        self.exec("UPDATE feedback SET state = 'deleting' WHERE state = 'sending' AND attempt_at <= ?", vec![SqlStorageValue::Integer(now - SENDING_TTL_MS)])?;
        let batch = SqlStorageValue::Integer(SWEEP_BATCH.into());
        let doomed = self.rows::<Doomed>(
            "SELECT id, person, attempt, fenced_until FROM feedback WHERE state = 'deleting' AND COALESCE(fenced_until, 0) <= ? ORDER BY n LIMIT ?",
            vec![SqlStorageValue::Integer(now), batch.clone()],
        )?;
        assert!(doomed.len() <= SWEEP_BATCH as usize, "a run deletes at most a batch");
        for d in &doomed {
            // every key under it, whatever try put it: none can land now
            let (_, more) = js::blob_delete_under(&self.env, &rules::feedback_prefix(&d.person, &d.id), 1).await?;
            assert!(!more, "a feedback's screenshots are a page's few");
            self.exec("DELETE FROM feedback WHERE id = ? AND state = 'deleting'", vec![d.id.as_str().into()])?;
        }
        let pruned = self.rows::<Doomed>(
            "SELECT id, person, attempt, fenced_until FROM feedback WHERE state = 'kept' AND fenced_until <= ? ORDER BY fenced_until LIMIT ?",
            vec![SqlStorageValue::Integer(now), batch],
        )?;
        assert!(pruned.len() <= SWEEP_BATCH as usize, "a run prunes at most a batch");
        for p in &pruned {
            let (keys, more) = js::blob_list(&self.env, &rules::feedback_prefix(&p.person, &p.id)).await?;
            assert!(!more, "a feedback's tries' screenshots are a page's few");
            let own = rules::attempt_prefix(&p.person, &p.id, &p.attempt);
            let stale: Vec<String> = keys.into_iter().filter(|k| !k.starts_with(&own)).collect();
            if !stale.is_empty() {
                js::blob_delete(&self.env, &stale).await?;
            }
            let fence = p.fenced_until.map_or(SqlStorageValue::Null, SqlStorageValue::Integer);
            self.exec("UPDATE feedback SET fenced_until = NULL WHERE id = ? AND state = 'kept' AND fenced_until = ?", vec![p.id.as_str().into(), fence])?;
        }
        match self.feedback_due(js::now_ms())? {
            Some(at) => self.arm_feedback(at).await,
            None => Ok(()),
        }
    }

    /// The test hook's `FeedbackAge`: a feedback's latest try, and its
    /// fence, made `ms` older, and the alarm armed now. Answers how many.
    pub(super) async fn feedback_age(&self, id: &str, ms: i64) -> CellResult<usize> {
        if !rules::valid_id(id) || !(0..=SENDING_TTL_MS + FENCE_MS).contains(&ms) {
            return Err(CellError::invalid("a feedback's id, and at most an hour and its fence of ms"));
        }
        let aged = self.rows::<IgnoredAny>(
            "UPDATE feedback SET attempt_at = attempt_at - ?, fenced_until = fenced_until - ? WHERE id = ? RETURNING id",
            vec![SqlStorageValue::Integer(ms), SqlStorageValue::Integer(ms), id.into()],
        )?;
        self.arm_feedback(js::now_ms()).await?;
        Ok(aged.len())
    }

    /// The latest a byte of the person's feedback can land, when it is to
    /// come (a wipe waits it out before it deletes their bytes).
    pub(super) fn feedback_fenced_until(&self, person: &str) -> CellResult<Option<i64>> {
        #[derive(Deserialize)]
        struct Latest {
            at: Option<i64>,
        }
        let r = self.row::<Latest>("SELECT MAX(fenced_until) AS at FROM feedback WHERE person = ? AND fenced_until > ?", vec![person.into(), SqlStorageValue::Integer(js::now_ms())])?;
        Ok(r.and_then(|r| r.at))
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
            FeedbackKeep::PATH => reply::<FeedbackKeep>(match body(bytes) {
                Ok(b) => self.feedback_keep(b).await,
                Err(e) => Err(e),
            }),
            FeedbackMailed::PATH => reply::<FeedbackMailed>(body::<FeedbackMailed>(bytes).and_then(|b| self.feedback_mailed(&b.id))),
            FeedbackList::PATH => reply::<FeedbackList>(body(bytes).and_then(|b| self.feedback_list(b))),
            FeedbackGet::PATH => reply::<FeedbackGet>(body::<FeedbackGet>(bytes).and_then(|b| self.feedback_got(&b.id))),
            FeedbackResolve::PATH => reply::<FeedbackResolve>(body(bytes).and_then(|b| self.feedback_resolve(b))),
            FeedbackDelete::PATH => reply::<FeedbackDelete>(match body(bytes) {
                Ok(b) => self.feedback_delete(b).await,
                Err(e) => Err(e),
            }),
            _ => return None,
        })
    }
}
