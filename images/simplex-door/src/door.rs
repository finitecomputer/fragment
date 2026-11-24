//! The runner's one peer: its door (the Durable Object that runs its
//! container), at `DOOR_API` (`http://door.fragment.internal`, the
//! container's egress intercepted to its own door). Plain HTTP/1.1, each
//! call carrying this start's generation (`GENERATION_HEADER`), which the
//! door fences its saves by.
//!
//! - `GET /save`: a life begins: the generation it is given
//!   (`LIFE_HEADER`, every later call's), the save it restores
//!   (`SAVE_HEADER`, named back exactly), `MARK_HEADER` the door's newest
//!   mark, and `BEHIND_HEADER: 1` when the save's mark is older than it;
//!   204, none (a new door); 410, none left though it saved (the runner
//!   ends, never a new profile); 409, a newer start runs.
//! - `PUT /save?seq=<n>`: a save, `MARK_HEADER` the newest effect it holds;
//!   409 when a newer generation runs (this runner then ends).
//! - `POST /deliver`: one of its owner's messages (`Delivery`), taken with a
//!   2xx whose `MARK_HEADER` numbers it as an effect.
//! - `PUT /file/<sha256>`: a file its owner sent, received whole, for a
//!   delivery to name (`File`); 503 while the door holds as many as it may
//!   (given again later). `GET /file/<sha256>`: a file of a reply's, which
//!   its connector gave the door.
//! - `GET /relay?host=&port=`: a WebSocket to one of SimpleX's XFTP
//!   servers, its bytes untouched both ways (the core's file traffic, which
//!   a container cannot send itself: socks.rs in the bridge).
//! - `POST /unreadable {save, why}`: the save given will not restore (its
//!   bytes, never this container's disk); the runner then ends (exit 2)
//!   and its door starts it from an older one.
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
use serde::{Deserialize, Serialize};

use crate::save::UNPACKED_MAX_BYTES;

pub const GENERATION_HEADER: &str = "x-door-generation";
pub const MARK_HEADER: &str = "x-door-mark";
pub const BEHIND_HEADER: &str = "x-door-behind";
pub const LIFE_HEADER: &str = "x-door-life";
pub const SAVE_HEADER: &str = "x-door-save";
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

/// What a start restores, and the generation its life is given
/// (`LIFE_HEADER`): every call after carries that one.
#[derive(Debug, PartialEq, Eq)]
pub enum Restore {
    /// No save: a new door.
    None { life: u64 },
    /// `save` named as its door named it, to name it back if it will not
    /// restore.
    Save { packed: Bytes, mark: u64, behind: bool, save: String, life: u64 },
    /// It saved, and no save of it restores: it never starts as a new door.
    Exhausted,
    /// A newer start runs: this one ends.
    Superseded,
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
    /// `text`, `image`, `voice`, `file`, `link`, …: any but text's text is
    /// its caption.
    pub kind: &'a str,
    /// Its text, and a note of a file it carried that did not come.
    pub text: String,
    /// When its sender sent it, as the core wrote it.
    pub at: &'a str,
    /// The file it carries, which the door holds (`PUT /file`).
    pub file: Option<&'a File>,
}

/// A file the door holds, by its SHA-256: one its owner sent, or one of a
/// reply's (as its connector names it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub sha256: String,
    pub name: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub size: u64,
}

#[derive(Serialize)]
struct Unreadable<'a> {
    save: &'a str,
    why: &'a str,
}

/// One answer of the door's, read.
#[derive(Default)]
struct Answer {
    status: StatusCode,
    mark: Option<u64>,
    behind: Option<bool>,
    life: Option<u64>,
    save: Option<String>,
    body: Bytes,
}

impl<'a> Delivery<'a> {
    /// `i`, carrying `file`, or with `note` (why its file did not come).
    pub fn of(i: &'a Inbound, file: Option<&'a File>, note: Option<&str>) -> Delivery<'a> {
        let text = match note {
            Some(n) if i.text.trim().is_empty() => n.to_string(),
            Some(n) => format!("{}\n\n{n}", i.text.trim_end()),
            None => i.text.clone(),
        };
        Delivery { id: &i.id, kind: &i.kind, text, at: &i.at, file }
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

    /// This door's calls as a life of generation `life` (its restore's).
    pub fn living(&self, life: u64) -> Door {
        assert!(life > 0, "a life has a generation");
        Door { generation: life, ..self.clone() }
    }

    /// One call: its status, its mark (when it names one), the life and the
    /// save it names, and its body.
    async fn call(&self, method: Method, path: &str, headers: &[(&str, String)], body: Option<Bytes>) -> Result<Answer, DoorError> {
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
            if status == StatusCode::CONFLICT || status == StatusCode::GONE {
                return Ok(Answer { status, ..Answer::default() });
            }
            let header = |name: &str| res.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            let number = |name: &str| header(name).map(|m| m.parse::<u64>().map_err(|_| DoorError::Answer { status: status.as_u16(), body: format!("{name} that is no number: {m:?}") })).transpose();
            let (mark, life) = (number(MARK_HEADER)?, number(LIFE_HEADER)?);
            let behind = header(BEHIND_HEADER).map(|b| b == "1");
            let save = header(SAVE_HEADER);
            let body = Limited::new(res.into_body(), ANSWER_MAX_BYTES).collect().await.map_err(|e| DoorError::Transport(e.to_string()))?.to_bytes();
            Ok::<_, DoorError>(Answer { status, mark, behind, life, save, body })
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
        let a = self.call(Method::GET, "/save", &[], None).await?;
        let missing = |what: &str| DoorError::Answer { status: a.status.as_u16(), body: format!("a restore's answer with no {what}") };
        match a.status {
            StatusCode::NO_CONTENT => Ok(Restore::None { life: a.life.ok_or_else(|| missing("life"))? }),
            StatusCode::OK => Ok(Restore::Save {
                mark: a.mark.ok_or_else(|| missing("mark"))?,
                behind: a.behind.unwrap_or(false),
                save: a.save.clone().ok_or_else(|| missing("save"))?,
                life: a.life.ok_or_else(|| missing("life"))?,
                packed: a.body,
            }),
            StatusCode::GONE => Ok(Restore::Exhausted),
            StatusCode::CONFLICT => Ok(Restore::Superseded),
            _ => Err(Door::refused(a.status, &a.body)),
        }
    }

    pub async fn save(&self, seq: u64, mark: u64, packed: Vec<u8>) -> Result<Saved, DoorError> {
        assert!(seq > 0, "a save is numbered from 1 in each start");
        let headers = [("content-type", "application/octet-stream".to_string()), (MARK_HEADER, mark.to_string())];
        let a = self.call(Method::PUT, &format!("/save?seq={seq}"), &headers, Some(Bytes::from(packed))).await?;
        match a.status {
            StatusCode::OK => Ok(Saved::Taken),
            StatusCode::CONFLICT => Ok(Saved::Superseded),
            _ => Err(Door::refused(a.status, &a.body)),
        }
    }

    /// The save this start was given will not restore (it then ends, exit
    /// 2): its door gives it no start again. Best effort: a door that does
    /// not hear it gives the same save again, and is told again.
    pub async fn unreadable(&self, save: &str, why: &str) {
        let body = serde_json::to_vec(&Unreadable { save, why }).expect("a reason is JSON");
        let _ = self.call(Method::POST, "/unreadable", &[("content-type", "application/json".to_string())], Some(Bytes::from(body))).await;
    }

    /// A file its owner sent, whole, its type as its message says: held by
    /// the door once this answers.
    pub async fn put_file(&self, sha256: &str, media_type: &str, bytes: Bytes) -> Result<(), DoorError> {
        let a = self.call(Method::PUT, &format!("/file/{sha256}"), &[("content-type", media_type.to_string())], Some(bytes)).await?;
        match a.status.is_success() {
            true => Ok(()),
            false => Err(Door::refused(a.status, &a.body)),
        }
    }

    /// A file of a reply's that the door holds, whole.
    pub async fn file(&self, sha256: &str) -> Result<Bytes, DoorError> {
        let a = self.call(Method::GET, &format!("/file/{sha256}"), &[], None).await?;
        match a.status {
            StatusCode::OK => Ok(a.body),
            _ => Err(Door::refused(a.status, &a.body)),
        }
    }

    /// An XFTP relay to `host` (a name of letters, digits, `.` and `-`) on
    /// `port`, through its door (`GET /relay`, a WebSocket).
    pub async fn relay(&self, host: &str, port: u16) -> Result<fragment_bridge::net::ClientWs, String> {
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
            return Err(format!("{} is no host a relay names", fragment_bridge::simplex::wire::cut(host, 64)));
        }
        fragment_bridge::net::connect_ws(&self.base, &format!("/relay?host={host}&port={port}"), &[(GENERATION_HEADER, self.generation.to_string())]).await
    }

    /// One message delivered: its mark, the door's number for it.
    pub async fn deliver(&self, d: &Delivery<'_>) -> Result<u64, DoorError> {
        let body = serde_json::to_vec(d).expect("a delivery is JSON");
        let a = self.call(Method::POST, "/deliver", &[("content-type", "application/json".to_string())], Some(Bytes::from(body))).await?;
        match (a.status.is_success(), a.mark) {
            (true, Some(m)) => Ok(m),
            (true, None) => Err(DoorError::Answer { status: a.status.as_u16(), body: "a delivery taken with no mark".into() }),
            (false, _) => Err(Door::refused(a.status, &a.body)),
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
        let (asked, restored) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\nx-door-mark: 12\r\nx-door-behind: 1\r\nx-door-save: 7-3\r\nx-door-life: 8\r\ncontent-length: 3\r\n\r\nabc"), door.restore());
        assert!(asked.starts_with("GET /save HTTP/1.1\r\n"), "{asked}");
        assert!(!asked.to_ascii_lowercase().contains("content-length"), "a GET carries no length: {asked}");
        assert!(asked.contains("x-door-generation: 7\r\n"), "{asked}");
        assert_eq!(restored, Ok(Restore::Save { packed: Bytes::from_static(b"abc"), mark: 12, behind: true, save: "7-3".into(), life: 8 }));
        let (_, restored) = tokio::join!(one(&listener, "HTTP/1.1 204 No Content\r\nx-door-life: 9\r\n\r\n"), door.restore());
        assert_eq!(restored, Ok(Restore::None { life: 9 }));
        // none left though it saved; a newer start runs
        let (_, restored) = tokio::join!(one(&listener, "HTTP/1.1 410 Gone\r\ncontent-length: 0\r\n\r\n"), door.restore());
        assert_eq!(restored, Ok(Restore::Exhausted));
        let (_, restored) = tokio::join!(one(&listener, "HTTP/1.1 409 Conflict\r\ncontent-length: 0\r\n\r\n"), door.restore());
        assert_eq!(restored, Ok(Restore::Superseded));
        let living = door.living(8);
        let (asked, _) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"), living.save(1, 0, vec![1]));
        assert!(asked.contains("x-door-generation: 8\r\n"), "a life's calls carry its generation: {asked}");
        let (_, restored) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\nx-door-life: 8\r\ncontent-length: 3\r\n\r\nabc"), door.restore());
        assert!(matches!(restored, Err(DoorError::Answer { status: 200, .. })), "a save with no mark or name: {restored:?}");

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
        let delivery = Delivery::of(&inbound, None, None);
        let (asked, mark) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\nx-door-mark: 13\r\ncontent-length: 0\r\n\r\n"), door.deliver(&delivery));
        assert!(asked.ends_with(r#"{"id":"c2hhcmVk","kind":"text","text":"hi","at":"2026-11-20T10:00:00Z","file":null}"#), "{asked}");
        assert_eq!(mark, Ok(13));
        let (_, mark) = tokio::join!(one(&listener, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"), door.deliver(&delivery));
        assert!(matches!(mark, Err(DoorError::Answer { .. })), "a delivery taken with no mark: {mark:?}");
    }
}
