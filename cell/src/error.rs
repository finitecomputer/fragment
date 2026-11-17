//! The cell's one error type. Every refusal becomes a `proto::ErrorBody`
//! with the status its code names; nothing answers with a bare string.

use fragment_proto::{ErrorBody, ErrorCode, Reason};
use worker::Response;

#[derive(Debug)]
pub struct CellError {
    pub code: ErrorCode,
    pub message: String,
    /// Why, for the people who read it (`ErrorBody::reason`): billing's,
    /// seats' and orgs' refusals name theirs.
    pub reason: Option<Reason>,
    /// A direct release guard refusal, before the called object acted.
    /// Never reconstructed from an object's JSON answer: it may have acted
    /// before a call of its own was refused (routed.rs).
    pub release_refused: Option<String>,
    /// How long its caller waits before asking again (`ErrorBody::
    /// retry_after_ms`, and a `Retry-After` header in seconds), where known.
    pub retry_after_ms: Option<u64>,
}

impl CellError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> CellError {
        CellError { code, message: message.into(), reason: None, release_refused: None, retry_after_ms: None }
    }

    /// The refusal, naming its cause.
    pub fn because(mut self, reason: Reason) -> CellError {
        self.reason = Some(reason);
        self
    }

    /// The refusal, saying how long to wait before asking again.
    pub fn retry_after(mut self, ms: u64) -> CellError {
        self.retry_after_ms = Some(ms);
        self
    }

    pub fn invalid(message: impl Into<String>) -> CellError {
        CellError::new(ErrorCode::InvalidRequest, message)
    }

    pub fn host(message: impl Into<String>) -> CellError {
        CellError::new(ErrorCode::HostFailed, message)
    }

    pub fn too_large(what: &str, got: usize, max: usize) -> CellError {
        CellError::new(ErrorCode::TooLarge, format!("{what} is {got} bytes; the limit is {max}"))
    }

    pub fn response(&self) -> worker::Result<Response> {
        let body = ErrorBody { error: self.code, message: self.message.clone(), reason: self.reason, retry_after_ms: self.retry_after_ms };
        let mut resp = Response::from_json(&body)?.with_status(self.code.status());
        if let Some(ms) = self.retry_after_ms {
            resp.headers_mut().set("retry-after", &ms.div_ceil(1000).to_string())?;
        }
        Ok(resp)
    }
}

impl From<worker::Error> for CellError {
    fn from(e: worker::Error) -> CellError {
        CellError::host(e.to_string())
    }
}

/// A seat Stripe does not sell, asked for (a buyer's, an org admin's or an
/// operator's trial code): 400.
impl From<fragment_core::stripe::NotSold> for CellError {
    fn from(e: fragment_core::stripe::NotSold) -> CellError {
        CellError::invalid(e.message()).because(Reason::NotSold)
    }
}

/// A scheduled record its bounds refuse (`fragment_core::schedule`): 400,
/// 403 below its channel's post role, or 413 for its record's size, saying
/// which.
impl From<fragment_core::schedule::Refusal> for CellError {
    fn from(r: fragment_core::schedule::Refusal) -> CellError {
        CellError::new(r.code(), r.to_string())
    }
}

pub type CellResult<T> = Result<T, CellError>;
