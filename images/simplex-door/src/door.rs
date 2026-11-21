//! The runner's one peer: its door (the Durable Object that runs its
//! container), at `DOOR_API` (`http://door.fragment.internal`, the
//! container's egress intercepted to its own door). Plain HTTP/1.1, each
//! call carrying this start's generation (`GENERATION_HEADER`), which the
//! door fences its saves by.
//!
//! - `GET /save`: the newest save, `MARK_HEADER` the door's newest mark,
//!   and `BEHIND_HEADER: 1` when the save's mark is older than it; 204,
//!   none (a new door).
//! - `PUT /save?seq=<n>`: a save, `MARK_HEADER` the newest effect it holds;
//!   409 when a newer generation runs (this runner then ends).
//! - `POST /deliver`: one of its owner's messages (`Delivery`), taken with a
//!   2xx whose `MARK_HEADER` numbers it as an effect.
//!
//! An effect is what moves a ratchet past a save: a send the door asked
//! for, or a message delivered. The door numbers each (its mark) as it
//! asks or takes it, so it knows a save is behind with no clock: a save's
//! mark is the newest the runner knew committed in it (main.rs, "marks").

use std::time::Duration;

use bytes::Bytes;
use fragment_bridge::simplex::link::Inbound;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde::Serialize;

use crate::save::UNPACKED_MAX_BYTES;

pub const GENERATION_HEADER: &str = "x-door-generation";
pub const MARK_HEADER: &str = "x-door-mark";
pub const BEHIND_HEADER: &str = "x-door-behind";
/// One call's wait for its whole answer.
const CALL_MS_MAX: u64 = 30_000;
/// A packed save is smaller than what it unpacks to (gzip never grows it by
/// more than its framing): an answer past this is no save.
const ANSWER_MAX_BYTES: usize = UNPACKED_MAX_BYTES as usize + 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum DoorError {
    /// It did not answer (connect, a timeout, a cut answer): asked again.
    Transport(String),
    /// It answered what this runner does not ask for, or refused.
    Answer { status: u16, body: String },
}

impl std::fmt::Display for DoorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DoorError::Transport(e) => write!(f, "the door did not answer: {e}"),
            DoorError::Answer { status, body } => write!(f, "the door answered {status}: {body}"),
        }
    }
}

/// What a start restores.
#[derive(Debug, PartialEq, Eq)]
pub enum Restore {
    /// No save: a new door.
    None,
    Save { packed: Bytes, mark: u64, behind: bool },
}

/// A save's answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Taken,
    /// A newer generation runs: this one must end, and never save again.
    Superseded,
}

/// One message for the connector's chat, flat. Its `id` is its sender's
/// (`itemSharedMsgId`): the same however often it is delivered, which the
/// door dedupes by.
#[derive(Debug, Serialize)]
pub struct Delivery<'a> {
    pub id: &'a str,
    /// `text`, `image`, `voice`, `file`, `link`, …: only text is carried;
    /// any other's text is its caption.
    pub kind: &'a str,
    pub text: &'a str,
    /// When its sender sent it, as the core wrote it.
    pub at: &'a str,
    /// A file it carries, named, not yet carried itself.
    pub file: Option<FileNote<'a>>,
}

#[derive(Debug, Serialize)]
pub struct FileNote<'a> {
    pub name: &'a str,
    pub size: u64,
}

impl<'a> Delivery<'a> {
    pub fn of(i: &'a Inbound) -> Delivery<'a> {
        Delivery { id: &i.id, kind: &i.kind, text: &i.text, at: &i.at, file: i.file.as_ref().map(|f| FileNote { name: &f.name, size: f.size }) }
    }
}

#[derive(Clone)]
pub struct Door {
    base: fragment_bridge::net::Base,
    generation: u64,
    client: Client<HttpConnector, Full<Bytes>>,
}

impl Door {
    pub fn new(url: &str, generation: u64) -> Result<Door, String> {
        assert!(generation > 0, "a start has a generation");
        let base = fragment_bridge::net::Base::parse(url)?;
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        connector.set_connect_timeout(Some(Duration::from_millis(CALL_MS_MAX)));
        let client = Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(30)).build(connector);
        Ok(Door { base, generation, client })
    }

    /// One call: its status, its mark (when it names one) and its body.
    async fn call(&self, method: Method, path: &str, headers: &[(&str, String)], body: Option<Bytes>) -> Result<(StatusCode, Option<u64>, Option<bool>, Bytes), DoorError> {
        let mut req = Request::builder().method(method).uri(self.base.url(path)).header("host", self.base.authority()).header(GENERATION_HEADER, self.generation.to_string());
        for (k, v) in headers {
            req = req.header(*k, v.as_str());
        }
        // a GET carries no body, and so no length: a `content-length: 0` on
        // one made the door's egress throw (a `Request` with a body on a
        // GET), and the first hosted door sat 510 s restoring (the spike)
        let body = match body {
            Some(b) => {
                req = req.header("content-length", b.len());
                Full::new(b)
            }
            None => Full::new(Bytes::new()),
        };
        let req = req.body(body).map_err(|e| DoorError::Transport(e.to_string()))?;
        let call = async {
            let res = self.client.request(req).await.map_err(|e| DoorError::Transport(e.to_string()))?;
            let status = res.status();
            // a fence is its status alone: taken from the head, whatever its
            // body does (one cut short or stalled must never read as a
            // failure to retry: the runner ends at once)
            if status == StatusCode::CONFLICT {
                return Ok((status, None, None, Bytes::new()));
            }
            let header = |name: &str| res.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            let mark = header(MARK_HEADER).map(|m| m.parse::<u64>().map_err(|_| DoorError::Answer { status: status.as_u16(), body: format!("a mark that is no number: {m:?}") })).transpose()?;
            let behind = header(BEHIND_HEADER).map(|b| b == "1");
            let bytes = Limited::new(res.into_body(), ANSWER_MAX_BYTES).collect().await.map_err(|e| DoorError::Transport(e.to_string()))?.to_bytes();
            Ok::<_, DoorError>((status, mark, behind, bytes))
        };
        match tokio::time::timeout(Duration::from_millis(CALL_MS_MAX), call).await {
            Ok(r) => r,
            Err(_) => Err(DoorError::Transport(format!("{path}: no answer in {CALL_MS_MAX} ms"))),
        }
    }

    fn refused(status: StatusCode, body: &Bytes) -> DoorError {
        DoorError::Answer { status: status.as_u16(), body: fragment_bridge::simplex::wire::cut(&String::from_utf8_lossy(body), 300).to_string() }
    }

    pub async fn restore(&self) -> Result<Restore, DoorError> {
        let (status, mark, behind, body) = self.call(Method::GET, "/save", &[], None).await?;
        match status {
            StatusCode::NO_CONTENT => Ok(Restore::None),
            StatusCode::OK => {
                let mark = mark.ok_or_else(|| DoorError::Answer { status: 200, body: "a save with no mark".into() })?;
                Ok(Restore::Save { packed: body, mark, behind: behind.unwrap_or(false) })
            }
            _ => Err(Door::refused(status, &body)),
        }
    }

    pub async fn save(&self, seq: u64, mark: u64, packed: Vec<u8>) -> Result<Saved, DoorError> {
        assert!(seq > 0, "a save is numbered from 1 in each start");
        let headers = [("content-type", "application/octet-stream".to_string()), (MARK_HEADER, mark.to_string())];
        let (status, _, _, body) = self.call(Method::PUT, &format!("/save?seq={seq}"), &headers, Some(Bytes::from(packed))).await?;
        match status {
            StatusCode::OK => Ok(Saved::Taken),
            StatusCode::CONFLICT => Ok(Saved::Superseded),
            _ => Err(Door::refused(status, &body)),
        }
    }

    /// One message delivered: its mark, the door's number for it.
    pub async fn deliver(&self, d: &Delivery<'_>) -> Result<u64, DoorError> {
        let body = serde_json::to_vec(d).expect("a delivery is JSON");
        let (status, mark, _, answer) = self.call(Method::POST, "/deliver", &[("content-type", "application/json".to_string())], Some(Bytes::from(body))).await?;
        match (status.is_success(), mark) {
            (true, Some(m)) => Ok(m),
            (true, None) => Err(DoorError::Answer { status: status.as_u16(), body: "a delivery taken with no mark".into() }),
            (false, _) => Err(Door::refused(status, &answer)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One request read whole from a socket, then `answer` written.
    async fn one(listener: &tokio::net::TcpListener, answer: &str) -> String {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = Vec::new();
        // bounded by the test's one small request
        loop {
            let n = s.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let len = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                if got.len() >= head_end + 4 + len {
                    break;
                }
            }
            assert!(n > 0, "the request ended early: {text}");
        }
        s.write_all(answer.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&got).to_string()
    }

    /// Goal: a restore's GET carries no body and no length (the egress bug
    /// the spike met), every call its generation, and each answer reads as
    /// the protocol says: a save with its mark and behind, none, a save
    /// taken or superseded, a delivery's mark, and refusals as refusals.
    /// Method: a socket standing in for the door, one call at a time.
    #[tokio::test]
    async fn the_door_is_called_as_its_protocol_says() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let door = Door::new(&format!("http://{}", listener.local_addr().unwrap()), 7).unwrap();
        let (asked, restored) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\nx-door-mark: 12\r\nx-door-behind: 1\r\ncontent-length: 3\r\n\r\nabc"), door.restore());
        assert!(asked.starts_with("GET /save HTTP/1.1\r\n"), "{asked}");
        assert!(!asked.to_ascii_lowercase().contains("content-length"), "a GET carries no length: {asked}");
        assert!(asked.contains("x-door-generation: 7\r\n"), "{asked}");
        assert_eq!(restored, Ok(Restore::Save { packed: Bytes::from_static(b"abc"), mark: 12, behind: true }));
        let (_, restored) = tokio::join!(one(&listener, "HTTP/1.1 204 No Content\r\n\r\n"), door.restore());
        assert_eq!(restored, Ok(Restore::None));
        let (_, restored) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nabc"), door.restore());
        assert!(matches!(restored, Err(DoorError::Answer { status: 200, .. })), "a save with no mark: {restored:?}");

        let (asked, saved) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"), door.save(3, 12, vec![1, 2]));
        assert!(asked.starts_with("PUT /save?seq=3 HTTP/1.1\r\n") && asked.contains("x-door-mark: 12\r\n") && asked.contains("content-length: 2\r\n"), "{asked}");
        assert_eq!(saved, Ok(Saved::Taken));
        let (_, saved) = tokio::join!(one(&listener, "HTTP/1.1 409 Conflict\r\ncontent-length: 0\r\n\r\n"), door.save(4, 12, vec![1]));
        assert_eq!(saved, Ok(Saved::Superseded));
        // a fence whose body is cut short is a fence still (review of #1430)
        let (_, saved) = tokio::join!(one(&listener, "HTTP/1.1 409 Conflict\r\ncontent-length: 1000\r\n\r\nabc"), door.save(4, 12, vec![1]));
        assert_eq!(saved, Ok(Saved::Superseded));
        let (_, saved) = tokio::join!(one(&listener, "HTTP/1.1 500 Oops\r\ncontent-length: 4\r\n\r\ngone"), door.save(5, 12, vec![1]));
        assert_eq!(saved, Err(DoorError::Answer { status: 500, body: "gone".into() }));

        let inbound = Inbound { id: "c2hhcmVk".into(), kind: "text".into(), text: "hi".into(), at: "2026-11-20T10:00:00Z".into(), item: 9, file: None };
        let delivery = Delivery::of(&inbound);
        let (asked, mark) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\nx-door-mark: 13\r\ncontent-length: 0\r\n\r\n"), door.deliver(&delivery));
        assert!(asked.ends_with(r#"{"id":"c2hhcmVk","kind":"text","text":"hi","at":"2026-11-20T10:00:00Z","file":null}"#), "{asked}");
        assert_eq!(mark, Ok(13));
        let (_, mark) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"), door.deliver(&delivery));
        assert!(matches!(mark, Err(DoorError::Answer { .. })), "a delivery taken with no mark: {mark:?}");
    }
}
