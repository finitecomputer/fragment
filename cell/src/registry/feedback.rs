//! Feedback (docs/api.md, Feedback): what people send the deployment's
//! operators from the shell, kept here beside the operators' log. A
//! person sends at most `PER_DAY_MAX` a day, and the registry keeps at
//! most `KEPT_MAX` (past it a new one waits for an operator to delete
//! some). Its screenshots' bytes are BLOBS', put by the router between
//! `FeedbackRoom` and `FeedbackSend` (cell/src/feedback.rs); a wipe of
//! the person deletes both (wipe.rs). The router checked who asks of the
//! operators' calls.

use fragment_core::feedback::{self as rules, DAY_MS, KEPT_MAX, PER_DAY_MAX};
use fragment_proto::feedback::{Feedback, FeedbackKind, FeedbackPage, Screenshot};
use serde::Serialize;

use super::calls::Call;
use super::*;

/// Feedback a page of the admin's lists: each holds up to 5,000 characters.
const PAGE: u32 = 50;

/// `POST /feedback/room`: whether whoever asks may send one now: a person,
/// under the day's count, and the registry not full. Answers who they are.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackRoom {
    pub by: By,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackFrom {
    pub person: String,
}

impl Call for FeedbackRoom {
    const PATH: &'static str = "/feedback/room";
    type Answer = FeedbackFrom;
}

/// `POST /feedback/send`: one kept, its screenshots' bytes put already.
/// Sent again with the same `id`, it answers the one kept.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackSend {
    pub by: By,
    pub id: String,
    pub kind: FeedbackKind,
    pub message: String,
    pub page: Option<String>,
    pub user_agent: Option<String>,
    pub release: String,
    pub screenshots: Vec<Screenshot>,
}

impl Call for FeedbackSend {
    const PATH: &'static str = "/feedback/send";
    type Answer = Feedback;
}

/// `POST /feedback/list`: a page, newest first, before `before`; only
/// those not done when `open`.
#[derive(Serialize, Deserialize)]
pub(crate) struct FeedbackList {
    pub before: Option<u64>,
    pub open: bool,
}

impl Call for FeedbackList {
    const PATH: &'static str = "/feedback/list";
    type Answer = FeedbackPage;
}

/// `POST /feedback/get`: one, by its id.
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

/// `POST /feedback/delete`: an operator (`by`) deletes one; answers it as
/// it was, for the router to delete its screenshots' bytes. Logged.
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
}

const SELECT: &str = "SELECT n, id, person, email, kind, message, page, user_agent, release, screenshots, created_at, resolved_at, resolved_by FROM feedback";

impl Row {
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

impl RegistryCell {
    /// The person who asks, if they may send one now.
    fn feedback_room(&self, by: &By) -> CellResult<Identity> {
        let who = self.by(by)?;
        if who.kind != IdentityKind::Person {
            return Err(CellError::new(ErrorCode::Forbidden, "feedback is a person's: an agent's owner sends it"));
        }
        let today = self.count("SELECT COUNT(*) AS n FROM feedback WHERE person = ? AND created_at > ?", vec![who.id.as_str().into(), SqlStorageValue::Integer(js::now_ms() - DAY_MS)])?;
        if today >= PER_DAY_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, format!("you have sent {PER_DAY_MAX} in a day, as many as we take: send more tomorrow")));
        }
        if self.count("SELECT COUNT(*) AS n FROM feedback", vec![])? >= KEPT_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, "we can't take more feedback right now: try again later"));
        }
        Ok(who)
    }

    fn feedback_row(&self, id: &str) -> CellResult<Option<Row>> {
        if !rules::valid_id(id) {
            return Err(CellError::new(ErrorCode::NotFound, format!("no feedback {id:?}")));
        }
        self.row::<Row>(&format!("{SELECT} WHERE id = ?"), vec![id.into()])
    }

    fn feedback_of(&self, id: &str) -> CellResult<Feedback> {
        self.feedback_row(id)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no feedback {id}")))?.wire()
    }

    fn feedback_send(&self, b: FeedbackSend) -> CellResult<Feedback> {
        if !rules::valid_id(&b.id) {
            return Err(CellError::invalid("a feedback's id is 32 lowercase hex digits"));
        }
        // the same send again is the one kept: its sender's alone
        let who = self.by(&b.by)?;
        if let Some(kept) = self.feedback_row(&b.id)? {
            if kept.person != who.id {
                return Err(CellError::new(ErrorCode::AlreadyExists, format!("feedback {} is someone else's", b.id)));
            }
            return kept.wire();
        }
        let who = self.feedback_room(&b.by)?;
        // the router read the form (fragment_core::feedback::read); checked
        // again here, as what is stored is this cell's to keep sound
        let chars = b.message.chars().count();
        assert!(chars > 0 && chars <= rules::MESSAGE_MAX_CHARS, "the router sends a message read");
        assert!(b.page.as_deref().is_none_or(rules::valid_page), "the router sends a page read");
        assert!(b.screenshots.len() <= rules::SCREENSHOTS_MAX, "the router sends a few screenshots");
        let shots = serde_json::to_string(&b.screenshots).map_err(|e| CellError::host(e.to_string()))?;
        let null = |v: Option<String>| v.map_or(SqlStorageValue::Null, Into::into);
        self.exec(
            "INSERT INTO feedback (id, person, email, kind, message, page, user_agent, release, screenshots, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        self.feedback_of(&b.id)
    }

    fn feedback_list(&self, b: FeedbackList) -> CellResult<FeedbackPage> {
        let before = b.before.map_or(i64::MAX, |n| i64::try_from(n).unwrap_or(i64::MAX));
        let open = if b.open { " AND resolved_at IS NULL" } else { "" };
        let rows = self.rows::<Row>(&format!("{SELECT} WHERE n < ?{open} ORDER BY n DESC LIMIT ?"), vec![SqlStorageValue::Integer(before), SqlStorageValue::Integer(PAGE.into())])?;
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

    fn feedback_delete(&self, b: FeedbackDelete) -> CellResult<Feedback> {
        let was = self.feedback_of(&b.id)?;
        self.exec("DELETE FROM feedback WHERE id = ?", vec![b.id.as_str().into()])?;
        self.log_admin(&b.by, "feedback-delete", &b.id, &format!("{} from {}", was.kind.as_str(), was.email.as_deref().unwrap_or(&was.person)))?;
        Ok(was)
    }

    pub(super) fn feedback_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            FeedbackRoom::PATH => reply::<FeedbackRoom>(body::<FeedbackRoom>(bytes).and_then(|b| self.feedback_room(&b.by)).map(|who| FeedbackFrom { person: who.id })),
            FeedbackSend::PATH => reply::<FeedbackSend>(body(bytes).and_then(|b| self.feedback_send(b))),
            FeedbackList::PATH => reply::<FeedbackList>(body(bytes).and_then(|b| self.feedback_list(b))),
            FeedbackGet::PATH => reply::<FeedbackGet>(body::<FeedbackGet>(bytes).and_then(|b| self.feedback_of(&b.id))),
            FeedbackResolve::PATH => reply::<FeedbackResolve>(body(bytes).and_then(|b| self.feedback_resolve(b))),
            FeedbackDelete::PATH => reply::<FeedbackDelete>(body(bytes).and_then(|b| self.feedback_delete(b))),
            _ => return None,
        })
    }
}
