//! Just enough HTTP/1.1 for a few fixed routes: read one request head
//! (bounded in size and time) and, for a service that takes one, a body
//! of at most a given length by Content-Length; answer; close. pod-init's
//! routes are GETs and small JSON POSTs.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const MAX_HEAD: usize = 8 * 1024;
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
pub enum HttpError {
    Io(io::Error),
    TooLarge,
    /// Content-Length is over what the service takes.
    BodyTooLarge { length: u64, max: usize },
    Malformed(&'static str),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading request: {e}"),
            Self::TooLarge => write!(f, "request head over {MAX_HEAD} bytes"),
            Self::BodyTooLarge { length, max } => write!(f, "request body is {length} bytes; at most {max}"),
            Self::Malformed(why) => write!(f, "malformed request: {why}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// Read a request head; the body, if any, is ignored (GET only).
pub fn read_request(stream: impl Read) -> Result<Request, HttpError> {
    let mut reader = BufReader::new(stream);
    let head = read_head(&mut reader)?;
    parse_request_line(head.lines().next().unwrap_or_default())
}

/// Read a request head and a body of at most `max_body` bytes, sized by
/// Content-Length (no chunked encoding).
pub fn read_request_with_body(stream: impl Read, max_body: usize) -> Result<Request, HttpError> {
    let mut reader = BufReader::new(stream);
    let head = read_head(&mut reader)?;
    let mut req = parse_request_line(head.lines().next().unwrap_or_default())?;
    let length = content_length(&head)?;
    if length > max_body as u64 {
        return Err(HttpError::BodyTooLarge { length, max: max_body });
    }
    reader.take(length).read_to_end(&mut req.body).map_err(HttpError::Io)?;
    if req.body.len() as u64 != length {
        return Err(HttpError::Malformed("body shorter than Content-Length"));
    }
    Ok(req)
}

fn read_head(reader: &mut impl BufRead) -> Result<String, HttpError> {
    let mut head = Vec::new();
    loop {
        // Never buffer more than one byte past the limit, however long a line.
        let budget = (MAX_HEAD + 1 - head.len()) as u64;
        let mut line = Vec::new();
        let n = reader.take(budget).read_until(b'\n', &mut line).map_err(HttpError::Io)?;
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD {
            return Err(HttpError::TooLarge);
        }
        if n == 0 {
            return Err(HttpError::Malformed("connection closed before the end of the head"));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    String::from_utf8(head).map_err(|_| HttpError::Malformed("not UTF-8"))
}

/// Content-Length, 0 if absent. Two of them, or any Transfer-Encoding,
/// is refused rather than guessed at.
fn content_length(head: &str) -> Result<u64, HttpError> {
    let mut length = None;
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(HttpError::Malformed("Transfer-Encoding is not supported; send Content-Length"));
        }
        if name.eq_ignore_ascii_case("content-length") {
            let n = value.trim().parse().map_err(|_| HttpError::Malformed("Content-Length"))?;
            if length.replace(n).is_some() {
                return Err(HttpError::Malformed("two Content-Length headers"));
            }
        }
    }
    Ok(length.unwrap_or(0))
}

pub fn parse_request_line(line: &str) -> Result<Request, HttpError> {
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::Malformed("request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Malformed("not HTTP/1.x"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_owned(), v.to_owned())
        })
        .collect();
    Ok(Request { method: method.to_owned(), path: path.to_owned(), query, body: Vec::new() })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn bytes(body: Vec<u8>) -> Self {
        Self { status: 200, content_type: "application/octet-stream", body }
    }

    pub fn json(status: u16, body: Vec<u8>) -> Self {
        Self { status, content_type: "application/json", body }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        let mut body: String = body.into();
        body.push('\n');
        Self { status, content_type: "text/plain; charset=utf-8", body: body.into_bytes() }
    }

    pub fn write_to(&self, mut out: impl Write) -> io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Content Too Large",
            _ => "Internal Server Error",
        };
        write!(
            out,
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            self.status,
            self.content_type,
            self.body.len()
        )?;
        out.write_all(&self.body)?;
        out.flush()
    }
}

pub fn set_timeouts(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))
}

/// How a service reads requests: head only, or with a bounded body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    Ignored,
    UpTo(usize),
}

/// Serve one connection at a time until the listener fails (or after
/// `max_connections`, for tests). `handle` also answers requests that
/// could not be read, so each service words its own refusals.
pub fn serve(
    listener: &TcpListener,
    name: &str,
    body: Body,
    max_connections: Option<usize>,
    handle: &dyn Fn(Result<Request, HttpError>) -> Response,
) {
    let mut served = 0usize;
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{name}: accept: {e}");
                continue;
            }
        };
        let read = set_timeouts(&stream).map_err(HttpError::Io).and_then(|_| match body {
            Body::Ignored => read_request(&stream),
            Body::UpTo(max) => read_request_with_body(&stream, max),
        });
        let line = match &read {
            Ok(r) => format!("{} {}", r.method, r.path),
            Err(e) => format!("unreadable request ({e})"),
        };
        let response = handle(read);
        eprintln!("{name}: {line} -> {}", response.status);
        if let Err(e) = response.write_to(&mut stream) {
            eprintln!("{name}: write: {e}");
        }
        served += 1;
        if max_connections.is_some_and(|m| served >= m) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_get_with_a_query() {
        let r = read_request(&b"GET /quote?nonce=abcd&x HTTP/1.1\r\nHost: a\r\n\r\n"[..]).unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/quote");
        assert_eq!(r.param("nonce"), Some("abcd"));
        assert_eq!(r.param("x"), Some(""));
    }

    #[test]
    fn refuses_oversized_truncated_and_malformed_heads() {
        let big = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(matches!(read_request(big.as_bytes()), Err(HttpError::TooLarge)));
        assert!(matches!(read_request(&b"GET / HTTP/1.1\r\n"[..]), Err(HttpError::Malformed(_))));
        assert!(matches!(read_request(&b"GET /\r\n\r\n"[..]), Err(HttpError::Malformed(_))));
        assert!(matches!(read_request(&b"GET / SPDY/3\r\n\r\n"[..]), Err(HttpError::Malformed(_))));
    }

    fn post(headers: &str, body: &str) -> Vec<u8> {
        format!("POST /unlock HTTP/1.1\r\nHost: a\r\n{headers}\r\n{body}").into_bytes()
    }

    #[test]
    fn reads_a_body_by_content_length() {
        let r = read_request_with_body(&post("Content-Length: 4\r\n", "{}xx")[..], 16).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.body, b"{}xx");
        let none = read_request_with_body(&post("", "")[..], 16).unwrap();
        assert!(none.body.is_empty());
        assert!(read_request(&post("Content-Length: 4\r\n", "{}xx")[..]).unwrap().body.is_empty());
    }

    #[test]
    fn refuses_large_short_chunked_and_doubled_bodies() {
        let big = read_request_with_body(&post("Content-Length: 17\r\n", "")[..], 16);
        assert!(matches!(big, Err(HttpError::BodyTooLarge { length: 17, max: 16 })));
        let short = read_request_with_body(&post("Content-Length: 8\r\n", "{}")[..], 16);
        assert!(matches!(short, Err(HttpError::Malformed(_))));
        let chunked = read_request_with_body(&post("Transfer-Encoding: chunked\r\n", "")[..], 16);
        assert!(matches!(chunked, Err(HttpError::Malformed(_))));
        let two = read_request_with_body(&post("Content-Length: 1\r\ncontent-length: 1\r\n", "a")[..], 16);
        assert!(matches!(two, Err(HttpError::Malformed(_))));
        let junk = read_request_with_body(&post("Content-Length: -1\r\n", "")[..], 16);
        assert!(matches!(junk, Err(HttpError::Malformed(_))));
    }

    #[test]
    fn writes_length_and_close() {
        let mut out = Vec::new();
        Response::text(400, "bad nonce").write_to(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(text.contains("Content-Length: 10\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.ends_with("\r\n\r\nbad nonce\n"));
    }
}
