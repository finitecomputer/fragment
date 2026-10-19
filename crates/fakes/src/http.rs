//! A small blocking HTTP/1.1 server over std TCP, enough for a fake
//! service (one request per connection), and `post`, a client for the
//! fakes' own calls (the Stripe fake's deliveries) and their tests, on
//! reqwest.
//!
//! The server is ours rather than a crate's (tiny_http's, say) for what a
//! fake must also do that a server for real use does not: close a
//! connection with no answer at all (`Response::unanswered`, an answer
//! lost on its way back), where tiny_http answers 500 for a request it
//! drops, and hand the socket over after a head to write a body that
//! never ends (`Response::endless`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// The query's pairs in order, a repeated key's each time.
    pub pairs: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// A request as the server reads one: `target` its path and query (a
    /// fake's own tests drive its handler with it).
    pub fn new(method: &str, target: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
        let (path, q) = target.split_once('?').unwrap_or((target, ""));
        let pairs: Vec<(String, String)> = q
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(k), decode(v))
            })
            .collect();
        let headers = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
        Request { method: method.into(), path: decode(path), query: pairs.iter().cloned().collect(), pairs, headers, body: body.to_vec() }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Close the connection without writing an answer.
    pub unanswered: bool,
    /// After its head, the connection is this one's: a body's written as
    /// it goes (`endless`).
    pub upgrade: Option<Upgrade>,
}

/// What runs a connection its answer's head handed over.
pub type Upgrade = Box<dyn FnOnce(TcpStream) + Send>;

impl Response {
    /// No answer at all: the connection closes once the request is read (a
    /// request that was handled, and an answer lost on its way back).
    pub fn unanswered() -> Response {
        Response { status: 0, headers: Vec::new(), body: Vec::new(), unanswered: true, upgrade: None }
    }

    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response::bytes(status, "application/json", v.to_string().into_bytes())
    }

    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Response {
        Response { status, headers: vec![("content-type".into(), content_type.into())], body, unanswered: false, upgrade: None }
    }

    /// A 200 whose chunked body never ends: written until its reader stops
    /// reading, so a limit that counts a body only once it is whole is never met.
    pub fn endless() -> Response {
        const CHUNK: usize = 64 * 1024;
        let chunk = format!("{CHUNK:x}\r\n{}\r\n", "x".repeat(CHUNK));
        let write = move |mut s: TcpStream| while s.write_all(chunk.as_bytes()).is_ok() {};
        let headers = vec![("content-type".into(), "text/plain".into()), ("transfer-encoding".into(), "chunked".into())];
        Response { status: 200, headers, body: Vec::new(), unanswered: false, upgrade: Some(Box::new(write)) }
    }

    pub fn with_header(mut self, k: &str, v: &str) -> Response {
        self.headers.push((k.into(), v.into()));
        self
    }
}

pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// Serves until dropped.
pub struct Server {
    pub url: String,
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// `port` 0 picks a free one.
    pub fn start(port: u16, handler: Handler) -> std::io::Result<Server> {
        Server::serve(TcpListener::bind(("127.0.0.1", port))?, handler)
    }

    /// Serves on a listener the caller bound, when it must know the port
    /// before the handler exists: binding once means no one else can take
    /// the port in between.
    pub fn serve(listener: TcpListener, handler: Handler) -> std::io::Result<Server> {
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let handler = Arc::clone(&handler);
                std::thread::spawn(move || serve_one(stream, &handler));
            }
        });
        Ok(Server { url: format!("http://127.0.0.1:{port}"), port, stop })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // wake the accept loop
    }
}

fn serve_one(mut stream: TcpStream, handler: &Handler) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Some(req) = read_request(&mut stream) else { return };
    let head = req.method == "HEAD";
    let resp = handler(&req);
    if resp.unanswered {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return;
    }
    if let Some(run) = resp.upgrade {
        let mut out = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
        for (k, v) in &resp.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("\r\n");
        if stream.write_all(out.as_bytes()).is_ok() {
            let _ = stream.set_read_timeout(None);
            run(stream);
        }
        return;
    }
    let mut out = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    // a HEAD's answer declares the length its GET's body has
    out.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", resp.body.len()));
    let _ = stream.write_all(out.as_bytes());
    if !head {
        let _ = stream.write_all(&resp.body);
    }
    let _ = stream.flush();
}

fn reason(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        _ => "Status",
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 16384];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 1024 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    let mut r = Request::new(&method, &target, &[], &body);
    r.headers = headers;
    Some(r)
}

/// Percent-decoding (and `+` as space in queries, which paths never carry).
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                }
                None => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The most of an answer's body `post` reads: a test's answer is a few
/// hundred bytes, and one past this is a bug, not a slow server.
pub const ANSWER_BYTES_MAX: usize = 1024 * 1024;

/// How long `post` waits for its connection, and then for each read.
const POST_TIMEOUT: Duration = Duration::from_secs(30);

/// How deep `post` reads an error's causes for its message.
const CAUSES_MAX: usize = 8;

/// POSTs to an `http://` URL; the answer's status once its body is read
/// whole, or why it failed (a refused connection, a head that is no HTTP,
/// a body cut short of its framing).
///
/// It returns as soon as the answer is complete, by its Content-Length or
/// its last chunk, rather than when the server closes: workerd's HTTP
/// server holds a connection open for a next request for 5 s after an
/// answer (KJ's pipeline timeout), whatever `connection: close` asked.
/// And each call has a connection of its own, never one kept from the call
/// before: workerd closes it after those 5 s, and a request written onto
/// it as it closes is lost (the e2e's `POOL_IDLE` says the same).
pub fn post(url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<u16, String> {
    if !url.starts_with("http://") {
        return Err("only http:// URLs".into());
    }
    let client = reqwest::blocking::Client::builder().pool_max_idle_per_host(0).connect_timeout(POST_TIMEOUT).timeout(POST_TIMEOUT).build().map_err(|e| why(&e))?;
    let mut request = client.post(url).body(body.to_vec());
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let answer = request.send().map_err(|e| why(&e))?;
    let status = answer.status().as_u16();
    let mut read = Vec::new();
    answer.take(ANSWER_BYTES_MAX as u64 + 1).read_to_end(&mut read).map_err(|e| format!("reading the body: {}", why(&e)))?;
    if read.len() > ANSWER_BYTES_MAX {
        return Err(format!("an answer of more than {ANSWER_BYTES_MAX} bytes"));
    }
    Ok(status)
}

/// An error and its causes, each after a colon: reqwest's own message
/// names only the request, its cause says what went wrong.
fn why(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cause = e.source();
    // bounded: CAUSES_MAX causes at most
    for _ in 0..CAUSES_MAX {
        let Some(c) = cause else { break };
        out.push_str(": ");
        out.push_str(&c.to_string());
        cause = c.source();
    }
    out
}

#[cfg(test)]
mod tests {
    //! `post` against servers of the tests' own, each of which answers and
    //! then holds the connection open, as workerd does for its pipeline
    //! timeout: a whole answer returns at once, however it is framed, and
    //! one cut short is an error, never a status.
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    /// A server that answers one request with `answer`, written in pieces
    /// of at most `piece` bytes, then holds the socket until the test ends
    /// (`hold`) or, with `close`, closes it.
    fn server(answer: &'static [u8], piece: usize, close: bool) -> (String, mpsc::Sender<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://127.0.0.1:{}/hook", listener.local_addr().unwrap().port());
        let (hold, held) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream).expect("a request");
            for part in answer.chunks(piece) {
                stream.write_all(part).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            if !close {
                // held until the test drops its end: never a close that ends the answer
                let _ = held.recv_timeout(Duration::from_secs(20));
            }
        });
        (url, hold)
    }

    fn timed(url: &str) -> (Result<u16, String>, Duration) {
        let t0 = Instant::now();
        let r = post(url, &[("content-type", "application/json")], b"{}");
        (r, t0.elapsed())
    }

    #[test]
    fn a_whole_answer_returns_at_once_on_a_connection_held_open() {
        let answers: [&'static [u8]; 4] = [
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\ncontent-type: application/json\r\n\r\n{\"ok\":true}",
            b"HTTP/1.1 202 Accepted\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\n{\"ok\r\n7\r\n\":true}\r\n0\r\nx-trailer: 1\r\n\r\n",
            b"HTTP/1.1 204 No Content\r\n\r\n",
            b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n",
        ];
        for (answer, status) in answers.into_iter().zip([200, 202, 204, 401]) {
            // in one write, and a byte at a time
            for piece in [answer.len(), 1] {
                let (url, hold) = server(answer, piece, false);
                let (r, took) = timed(&url);
                assert_eq!(r, Ok(status), "{}", String::from_utf8_lossy(answer));
                assert!(took < Duration::from_secs(3), "{took:?}: post waited for the server to close");
                drop(hold);
            }
        }
    }

    #[test]
    fn an_answer_without_a_length_is_read_until_the_server_closes() {
        let (url, _hold) = server(b"HTTP/1.1 200 OK\r\n\r\nall of it", 4, true);
        assert_eq!(timed(&url).0, Ok(200));
    }

    #[test]
    fn an_answer_cut_short_is_an_error() {
        let cut: [&'static [u8]; 4] = [
            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n{\"ok\":",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\nshort",
            b"HTTP/1.1 200 OK\r\ncontent-len",
            b"",
        ];
        for answer in cut {
            let (url, _hold) = server(answer, 64, true);
            let r = timed(&url).0;
            assert!(r.is_err(), "{r:?} for {}", String::from_utf8_lossy(answer));
        }
        let (url, _hold) = server(b"SMTP ready\r\n\r\n", 64, true);
        let r = timed(&url).0;
        assert!(r.as_ref().is_err_and(|e| e.contains("invalid HTTP version")), "{r:?}");
    }
}
