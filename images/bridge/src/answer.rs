//! An answer for the computer (docs/bridge.md, "An answer for the
//! computer"): a card's option its runtime asks to be answered on the
//! agent's computer, never in a record (`PromptOption::computer`: a
//! sign-in's code), on the computer's screen port (6080), reached through
//! the platform by its owner alone (docs/computers.md, Ports), as the
//! SimpleX link page is.
//!
//! - `GET /answer/`: its page (`answer.html`, in the bridge, so every image
//!   serves the same one), which the shell frames, unseen, through a port
//!   ticket, and hands the answer the chat's card took
//!   (templates/chat/site/frame-messages.js, `cardToComputer`); it posts it
//!   here, and tells the shell what came of it (`computerAnswered`).
//! - `POST /answer?agent=<fragment>`: `{turn, prompt, option, value}`,
//!   JSON from this page's own origin alone, at most `ANSWER_MAX_BYTES`: the
//!   engine's (`Input::ComputerAnswer`). 200 `{ok: true}` once it is taken;
//!   409 with why, in words its person reads.
//!
//! Its value is the runtime's alone: never logged, never kept here. The
//! port is its owner's through the platform; something on the computer
//! itself may call it too (the agent's own tools), which already holds what
//! such an answer opens, so an answer of its own can only fail.

use std::time::Duration;

use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::net::{self, Body};

/// A posted answer is at most this many bytes.
pub const ANSWER_MAX_BYTES: usize = 4096;

/// How long a post waits for the engine to take its answer.
const TAKEN_WAIT_MS: u64 = 10_000;

const PAGE: &str = include_str!("answer.html");

/// A posted answer, for the driver to step the engine with (`Input::
/// ComputerAnswer`), and where whether it was taken goes.
pub struct Ask {
    pub agent: String,
    pub turn: String,
    pub prompt: String,
    pub option: String,
    pub words: String,
    pub taken: oneshot::Sender<Result<(), String>>,
}

pub type Answers = mpsc::Sender<Ask>;

/// Whether `path` is this page's.
pub fn ours(path: &str) -> bool {
    path == "/answer" || path == "/answer/"
}

/// `GET /answer/` and `POST /answer`; `runs` says whether the bridge runs
/// an agent (none until it has read them).
pub async fn handle(req: Request<Incoming>, answers: Option<&Answers>, runs: impl Fn(&str) -> bool) -> Response<Body> {
    match (req.method().clone(), req.uri().path()) {
        (Method::GET, "/answer/") => net::respond(StatusCode::OK, "text/html; charset=utf-8", PAGE),
        (Method::POST, "/answer") => {
            let Some(answers) = answers else { return net::refusal(StatusCode::NOT_FOUND, "not_found", "this computer takes no answers") };
            let agent = match crate::simplex::http::agent_of(req.uri().query()) {
                Ok(a) => a,
                Err(why) => return net::refusal(StatusCode::BAD_REQUEST, "invalid", &why),
            };
            if !runs(&agent) {
                return net::refusal(StatusCode::NOT_FOUND, "not_found", &format!("no agent {agent} runs on this computer"));
            }
            // from this page alone: JSON (a form cannot send it), and no
            // other site's page (a browser says where a request comes from)
            let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            if !header("content-type").is_some_and(|t| t.starts_with("application/json")) {
                return net::refusal(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid", "send JSON");
            }
            if header("sec-fetch-site").is_some_and(|s| s != "same-origin") {
                return net::refusal(StatusCode::FORBIDDEN, "forbidden", "an answer comes from this page alone");
            }
            let Ok(bytes) = Limited::new(req.into_body(), ANSWER_MAX_BYTES).collect().await.map(|b| b.to_bytes()) else {
                return net::refusal(StatusCode::PAYLOAD_TOO_LARGE, "invalid", &format!("at most {ANSWER_MAX_BYTES} bytes"));
            };
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let field = |k: &str| v[k].as_str().filter(|s| !s.is_empty() && s.len() <= 256).map(str::to_string);
            let (Some(turn), Some(prompt), Some(option), Some(words)) = (field("turn"), field("prompt"), field("option"), v["value"].as_str().map(str::to_string)) else {
                return net::refusal(StatusCode::BAD_REQUEST, "invalid", "{turn, prompt, option, value}");
            };
            let (taken, said) = oneshot::channel();
            if answers.send(Ask { agent, turn, prompt, option, words, taken }).await.is_err() {
                return net::refusal(StatusCode::SERVICE_UNAVAILABLE, "starting", "this computer is starting: try again in a moment");
            }
            match tokio::time::timeout(Duration::from_millis(TAKEN_WAIT_MS), said).await {
                Ok(Ok(Ok(()))) => net::json_answer(StatusCode::OK, &json!({ "ok": true })),
                Ok(Ok(Err(why))) => net::refusal(StatusCode::CONFLICT, "not_taken", &why),
                _ => net::refusal(StatusCode::SERVICE_UNAVAILABLE, "busy", "this computer did not answer in time: try again"),
            }
        }
        _ => net::refusal(StatusCode::NOT_FOUND, "not_found", "GET /answer/; POST /answer"),
    }
}
