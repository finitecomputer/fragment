//! The cell's one error type. Every refusal becomes a `proto::ErrorBody`
//! with the status its code names; nothing answers with a bare string.

use fragment_core::public_error;
use fragment_proto::{ErrorBody, ErrorCode, Reason};
use worker::{console_error, Request, Response, ResponseBody};

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

    /// The refusal as one of the platform's objects answers the router,
    /// its whole message kept. Past the cell's edge, `public_response`.
    pub fn response(&self) -> worker::Result<Response> {
        self.answered(self.message.clone())
    }

    /// The refusal as whoever asked from outside reads it: the platform's
    /// own failure says a fixed sentence and `reference`, and its message
    /// goes to the Worker's logs under that reference alone
    /// (`fragment_core::public_error`).
    pub fn public_response(&self, reference: &str) -> worker::Result<Response> {
        if public_error::redacted(self.code).is_some() {
            logged(self.code, &self.message, reference);
        }
        self.answered(public_error::public_message(self.code, &self.message, reference))
    }

    fn answered(&self, message: String) -> worker::Result<Response> {
        let body = ErrorBody { error: self.code, message, reason: self.reason, retry_after_ms: self.retry_after_ms };
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

/// A request's reference (`public_error::reference`): its `Cf-Ray`, or a
/// random one where Cloudflare set none (a local fleet).
pub fn reference_of(req: &Request) -> worker::Result<String> {
    let ray = req.headers().get("cf-ray")?;
    Ok(public_error::reference(ray.as_deref(), &crate::js::random_hex::<8>()))
}

/// The most characters of a vendor's own words a log line keeps.
const VENDOR_SAID_MAX_CHARS: usize = 300;

/// What a vendor said when it refused a call, in the Worker's logs and
/// nowhere else: a refusal's message names the vendor and its status,
/// never its words (they may name its internals, an id, a path of ours),
/// so they reach no one outside the platform (docs/security/error-responses.md).
pub fn vendor_said(vendor: &str, call: &str, status: u16, said: &str) {
    let said: String = said.chars().take(VENDOR_SAID_MAX_CHARS).collect();
    console_error!("{}", serde_json::json!({ "vendor": vendor, "call": call, "status": status, "said": said }));
}

/// A redacted refusal's whole message, in the Worker's logs, under the
/// reference its asker was told.
fn logged(code: ErrorCode, message: &str, reference: &str) {
    console_error!("{}", serde_json::json!({ "refused": code, "reference": reference, "message": message }));
}

/// The bytes an answer's refusal may hold for `public_answer` to read it
/// (an `ErrorBody` is a code and a sentence); a longer body is no refusal
/// of the platform's and passes as it is.
const REFUSAL_READ_MAX_BYTES: usize = 64 * 1024;

/// An answer on its way out of the cell, with a refusal some object of the
/// platform's answered (and the router passed on as it was) said as
/// `public_response` says one: only a JSON `ErrorBody` of a redacted code,
/// at its status; every other answer is untouched, its body unread.
pub async fn public_answer(mut resp: Response, reference: &str) -> worker::Result<Response> {
    let status = resp.status_code();
    let json = resp.headers().get("content-type")?.is_some_and(|t| t.starts_with("application/json"));
    let redacted_status = [ErrorCode::HostFailed].iter().any(|c| c.status() == status);
    if !(json && redacted_status) {
        return Ok(resp);
    }
    let bytes = match resp.body() {
        ResponseBody::Empty => return Ok(resp),
        ResponseBody::Body(b) if b.len() <= REFUSAL_READ_MAX_BYTES => b.clone(),
        ResponseBody::Body(_) => return Ok(resp),
        // an object's answer: a copy read, bounded, and the answer kept whole
        ResponseBody::Stream(_) => match crate::cs::read_answer(&mut resp.cloned()?, REFUSAL_READ_MAX_BYTES).await {
            Ok((bytes, false)) => bytes,
            _ => return Ok(resp),
        },
    };
    let Ok(body) = serde_json::from_slice::<ErrorBody>(&bytes) else { return Ok(resp) };
    if body.error.status() != status || public_error::redacted(body.error).is_none() {
        return Ok(resp);
    }
    logged(body.error, &body.message, reference);
    let said = ErrorBody { message: public_error::public_message(body.error, &body.message, reference), ..body };
    let h = resp.headers().clone();
    h.delete("content-length")?;
    h.delete("content-encoding")?;
    h.delete("etag")?;
    Ok(Response::from_json(&said)?.with_status(status).with_headers(h))
}
