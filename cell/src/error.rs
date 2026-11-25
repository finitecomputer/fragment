//! The cell's one error type. Every refusal becomes a `proto::ErrorBody`
//! with the status its code names; nothing answers with a bare string.

use fragment_core::public_error;
use fragment_proto::{ErrorBody, ErrorCode, Reason};
use worker::{console_error, console_log, Headers, Request, Response, ResponseBody};

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
    /// its whole message kept, and marked as the platform's own with its
    /// code (`PLATFORM_REFUSAL`), so the edge tells it from an app's answer
    /// and says it from the mark (`public_answer`). Past the cell's edge,
    /// `public_response`.
    pub fn response(&self) -> worker::Result<Response> {
        let mut resp = self.answered(self.message.clone())?;
        resp.headers_mut().set(PLATFORM_REFUSAL, &public_error::mark(self.code))?;
        Ok(resp)
    }

    /// The refusal, its whole message kept and unmarked: a computer's
    /// egress's own (`computer::ComputerEgress::handle`), which only that
    /// computer's own agent reads (docs/security/error-responses.md).
    pub fn unmarked_response(&self) -> worker::Result<Response> {
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
/// `status` 0: no answer came (a fetch's or a binding's failure).
pub fn vendor_said(vendor: &str, call: &str, status: u16, said: &str) {
    let said: String = said.chars().take(VENDOR_SAID_MAX_CHARS).collect();
    console_error!("{}", serde_json::json!({ "vendor": vendor, "call": call, "status": status, "said": said }));
}

/// A redacted refusal's whole message, in the Worker's logs, under the
/// reference its asker was told.
fn logged(code: ErrorCode, message: &str, reference: &str) {
    console_error!("{}", serde_json::json!({ "refused": code, "reference": reference, "message": message }));
}

/// Marks a refusal one of the platform's objects answered (`response`):
/// the edge rereads only an answer with it (`public_answer`), and drops
/// it from every answer it passes. A foreign answer (an app's, a computer
/// image's, a vendor's passed through) is stripped of it where it enters
/// (`foreign`), so it can never pass for the platform's
/// (`public_error::MARK`).
pub const PLATFORM_REFUSAL: &str = public_error::MARK;

/// A foreign answer (an app's facet, a computer's port, a vendor's answer
/// passed through) as it enters the platform, or any answer as it leaves
/// for a computer's guest: without the platform's mark, which it may have
/// sent to pass its words off as the platform's refusal
/// (`public_error::forged`). Unchanged when it carries none (`has` is
/// case-blind, as every header lookup is); else its headers copied whole
/// but the mark (a fetched answer's are immutable), which keeps a socket's
/// 101 and its WebSocket.
pub fn foreign(resp: Response) -> worker::Result<Response> {
    if !resp.headers().has(PLATFORM_REFUSAL)? {
        return Ok(resp);
    }
    let names: Vec<String> = resp.headers().keys().collect();
    let forged = public_error::forged(names.iter().map(String::as_str));
    if forged.is_empty() {
        return Ok(resp);
    }
    console_log!("{}", serde_json::json!({ "foreign": "dropped", "headers": forged }));
    let h = Headers::new();
    for (k, v) in resp.headers().entries() {
        if !k.eq_ignore_ascii_case(PLATFORM_REFUSAL) {
            h.append(&k, &v)?;
        }
    }
    Ok(resp.with_headers(h))
}

/// The bytes of a marked refusal `public_answer` reads to log its message
/// (an `ErrorBody` is a code and a sentence); a longer one is logged
/// unread, and says the same sentence (`public_error::said`).
const REFUSAL_READ_MAX_BYTES: usize = 64 * 1024;

/// An answer on its way out of the cell, with a refusal some object of the
/// platform's answered (and the router passed on as it was) said as
/// `public_response` says one: only an answer the platform marked as its
/// own (`PLATFORM_REFUSAL`), and only by its mark's code
/// (`public_error::said`): a redacted one is answered as a fixed sentence
/// and `reference` whatever its body held, read only (bounded) for the
/// logs. Every other answer, an app's or a computer image's or a refusal
/// its asker acts on, keeps its body, unread, and loses only the mark.
pub async fn public_answer(mut resp: Response, reference: &str) -> worker::Result<Response> {
    let Some(mark) = resp.headers().get(PLATFORM_REFUSAL)? else { return Ok(resp) };
    if public_error::said(&mark, None, reference).is_none() {
        return unmarked(resp);
    }
    let bytes = match resp.body() {
        ResponseBody::Empty => None,
        ResponseBody::Body(b) if b.len() <= REFUSAL_READ_MAX_BYTES => Some(b.clone()),
        ResponseBody::Body(_) => None,
        // an object's answer, read bounded: it is answered anew below
        ResponseBody::Stream(_) => match crate::cs::read_answer(&mut resp, REFUSAL_READ_MAX_BYTES).await {
            Ok((bytes, false)) => Some(bytes),
            _ => None,
        },
    };
    let read = bytes.and_then(|b| serde_json::from_slice::<ErrorBody>(&b).ok());
    let Some(said) = public_error::said(&mark, read.as_ref(), reference) else { return unmarked(resp) };
    match read.as_ref().filter(|b| b.error == said.error) {
        Some(body) => logged(said.error, &body.message, reference),
        None => logged(said.error, &format!("(its body unread: over {REFUSAL_READ_MAX_BYTES} bytes, or no refusal of its mark's)"), reference),
    }
    let h = resp.headers().clone();
    for name in [PLATFORM_REFUSAL, "content-length", "content-encoding", "etag"] {
        h.delete(name)?;
    }
    h.set("content-type", "application/json")?;
    Ok(Response::from_json(&said)?.with_status(said.error.status()).with_headers(h))
}

/// The message a refusal the fragment answered one of its app's
/// capabilities (entry.mjs `Files`, `Search`) says to the app's code, which
/// may pass it on to anyone (its visitor, through `app_failed`): the
/// platform's own failure as the edge says it (`public_error::said`), its
/// whole message logged under a reference of its own; `None` where its
/// own message is the app's to read.
pub fn app_said(mark: Option<&str>, body: &str) -> Option<String> {
    let mark = mark?;
    let reference = crate::js::random_hex::<8>();
    let read = serde_json::from_slice::<ErrorBody>(body.as_bytes()).ok();
    let said = public_error::said(mark, read.as_ref(), &reference)?;
    let message = read.filter(|b| b.error == said.error).map(|b| b.message).unwrap_or_else(|| "(its body no refusal of its mark's)".to_string());
    logged(said.error, &message, &reference);
    Some(said.message)
}

/// `resp` without `PLATFORM_REFUSAL`: an object's answer has immutable
/// headers, so cloned, which keeps a socket's 101 and its WebSocket.
fn unmarked(resp: Response) -> worker::Result<Response> {
    let h = resp.headers().clone();
    h.delete(PLATFORM_REFUSAL)?;
    Ok(resp.with_headers(h))
}
