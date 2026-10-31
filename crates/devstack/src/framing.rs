//! What a front-door connection carries (front.rs), told apart as the bytes
//! pass: each request's head (`GET /x`) and each response's status, so the
//! node's log says what was asked and answered, as wrangler's proxy's lines
//! did before the front door passed it by.
//! It only watches: the bytes are passed on whatever it makes of them, and
//! anything it cannot follow (a head too long, a body it cannot frame, a
//! socket's frames after an upgrade) ends its watching of that direction.

use std::time::Duration;

/// The longest head followed.
const HEAD_MAX: usize = 64 * 1024;
/// The most headers a head is read with.
const HEADERS_MAX: usize = 96;

/// One direction of a connection, framed.
#[derive(Debug)]
pub(crate) struct Framer {
    side: Side,
    state: State,
    head: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Side {
    /// The client's requests.
    Requests,
    /// The runtime's responses.
    Responses,
}

#[derive(Debug, PartialEq)]
enum State {
    /// Reading a head.
    Head,
    /// Skipping a body of this many bytes more.
    Body(u64),
    /// Inside a chunked body.
    Chunked(Chunk),
    /// No longer followed: an upgrade's frames, a body to the close, or
    /// what it could not frame.
    Opaque,
}

#[derive(Debug, PartialEq)]
enum Chunk {
    /// Reading a chunk's size line (its bytes so far).
    Size(Vec<u8>),
    /// Skipping this many bytes of a chunk's data, then its CRLF.
    Data(u64),
    /// Skipping the CRLF after a chunk's data: how many of its 2 bytes left.
    DataEnd(u8),
    /// Reading the trailers after the last chunk (their bytes so far).
    Trailers(Vec<u8>),
}

/// A head the framer read.
#[derive(Debug, PartialEq)]
pub(crate) enum Head {
    Request { method: String, path: String },
    Response { status: u16 },
}

impl Framer {
    pub(crate) fn new(side: Side) -> Framer {
        Framer { side, state: State::Head, head: Vec::new() }
    }

    /// Bytes as they pass; each head read, as it is.
    pub(crate) fn feed(&mut self, mut bytes: &[u8], head_request: &dyn Fn() -> bool, mut heard: impl FnMut(Head)) {
        while !bytes.is_empty() {
            match &mut self.state {
                State::Opaque => return,
                State::Body(left) => {
                    let n = (*left).min(bytes.len() as u64);
                    *left -= n;
                    bytes = &bytes[n as usize..];
                    if *left == 0 {
                        self.state = State::Head;
                    }
                }
                State::Chunked(chunk) => {
                    let (used, done) = chunked(chunk, bytes);
                    bytes = &bytes[used..];
                    match done {
                        Some(true) => self.state = State::Head,
                        Some(false) => self.state = State::Opaque,
                        None => {}
                    }
                }
                State::Head => {
                    let before = self.head.len();
                    self.head.extend_from_slice(bytes);
                    let Some(end) = find(&self.head, b"\r\n\r\n") else {
                        if self.head.len() > HEAD_MAX {
                            self.state = State::Opaque;
                        }
                        return;
                    };
                    let head: Vec<u8> = self.head.drain(..end + 4).collect();
                    // what of `bytes` follows the head
                    bytes = &bytes[(end + 4 - before)..];
                    self.head.clear();
                    self.state = match self.read(&head, head_request) {
                        Some((h, next)) => {
                            heard(h);
                            next
                        }
                        None => State::Opaque,
                    };
                }
            }
        }
    }

    /// A head: what it says, and what comes after it.
    fn read(&self, head: &[u8], head_request: &dyn Fn() -> bool) -> Option<(Head, State)> {
        let mut headers = [httparse::EMPTY_HEADER; HEADERS_MAX];
        match self.side {
            Side::Requests => {
                let mut req = httparse::Request::new(&mut headers);
                req.parse(head).ok()?.is_complete().then_some(())?;
                let next = body(req.headers, true)?;
                Some((Head::Request { method: req.method?.to_string(), path: req.path?.to_string() }, next))
            }
            Side::Responses => {
                let mut resp = httparse::Response::new(&mut headers);
                resp.parse(head).ok()?.is_complete().then_some(())?;
                let status = resp.code?;
                let next = match status {
                    // a socket's frames from here
                    101 => State::Opaque,
                    100..=199 | 204 | 304 => State::Head,
                    _ if head_request() => State::Head,
                    _ => body(resp.headers, false)?,
                };
                Some((Head::Response { status }, next))
            }
        }
    }
}

/// What follows a head with `headers`: its body's framing. A request with
/// neither length has none; a response with neither runs to the close.
fn body(headers: &[httparse::Header], request: bool) -> Option<State> {
    let get = |name: &str| headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| std::str::from_utf8(h.value).ok());
    if let Some(te) = get("transfer-encoding") {
        return te?.to_ascii_lowercase().trim().ends_with("chunked").then_some(State::Chunked(Chunk::Size(Vec::new())));
    }
    match get("content-length") {
        Some(len) => match len?.trim().parse::<u64>().ok()? {
            0 => Some(State::Head),
            n => Some(State::Body(n)),
        },
        None if request => Some(State::Head),
        None => Some(State::Opaque),
    }
}

/// Follows a chunked body through `bytes`: how many it used, and whether
/// the body ended (`Some(true)`), cannot be followed (`Some(false)`), or
/// goes on (`None`).
fn chunked(chunk: &mut Chunk, bytes: &[u8]) -> (usize, Option<bool>) {
    let mut used = 0;
    while used < bytes.len() {
        let rest = &bytes[used..];
        match chunk {
            Chunk::Size(line) => match rest.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    line.extend_from_slice(&rest[..=i]);
                    used += i + 1;
                    let text = String::from_utf8_lossy(line);
                    let hex = text.trim_end_matches(['\r', '\n']).split(';').next().unwrap_or("").trim();
                    let Ok(size) = u64::from_str_radix(hex, 16) else { return (used, Some(false)) };
                    *chunk = if size == 0 { Chunk::Trailers(Vec::new()) } else { Chunk::Data(size) };
                }
                None => {
                    line.extend_from_slice(rest);
                    if line.len() > 1024 {
                        return (bytes.len(), Some(false));
                    }
                    return (bytes.len(), None);
                }
            },
            Chunk::Data(left) => {
                let n = (*left).min(rest.len() as u64);
                *left -= n;
                used += n as usize;
                if *left == 0 {
                    *chunk = Chunk::DataEnd(2);
                }
            }
            Chunk::DataEnd(left) => {
                let n = (*left as usize).min(rest.len());
                *left -= n as u8;
                used += n;
                if *left == 0 {
                    *chunk = Chunk::Size(Vec::new());
                }
            }
            Chunk::Trailers(seen) => {
                seen.extend_from_slice(rest);
                // the empty line that ends them: right away, or after trailers
                let end = if seen.starts_with(b"\r\n") { Some(2) } else { find(seen, b"\r\n\r\n").map(|i| i + 4) };
                match end {
                    Some(end) => {
                        // what of `rest` the trailers' end took
                        let took = end - (seen.len() - rest.len());
                        return (used + took, Some(true));
                    }
                    None if seen.len() > HEAD_MAX => return (bytes.len(), Some(false)),
                    None => return (bytes.len(), None),
                }
            }
        }
    }
    (used, None)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A request and its answer, as the node's log says it.
pub(crate) fn said(method: &str, path: &str, status: u16, took: Duration) -> String {
    let path: String = path.chars().take(200).collect();
    format!("[front] {method} {path} {status} ({}ms)\n", took.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heads(side: Side, pieces: &[&[u8]], head_request: bool) -> Vec<Head> {
        let mut f = Framer::new(side);
        let mut out = Vec::new();
        for p in pieces {
            f.feed(p, &|| head_request, |h| out.push(h));
        }
        out
    }

    fn req(method: &str, path: &str) -> Head {
        Head::Request { method: method.into(), path: path.into() }
    }

    /// Valid: requests one after another on a connection, with and
    /// without bodies, in pieces cut anywhere, each head read once.
    #[test]
    fn requests_kept_alive_are_each_read() {
        let stream = b"GET /a HTTP/1.1\r\nhost: x\r\n\r\nPUT /b HTTP/1.1\r\ncontent-length: 5\r\n\r\nhelloPOST /c HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\nGET /d HTTP/1.1\r\n\r\n";
        let want = vec![req("GET", "/a"), req("PUT", "/b"), req("POST", "/c"), req("GET", "/d")];
        assert_eq!(heads(Side::Requests, &[stream], false), want);
        for cut in 1..stream.len() {
            assert_eq!(heads(Side::Requests, &[&stream[..cut], &stream[cut..]], false), want, "cut at {cut}");
        }
        let bytes: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(heads(Side::Requests, &bytes, false), want, "a byte at a time");
    }

    /// Responses: lengths, chunks with trailers, none for 204/304 or a
    /// HEAD's answer; an upgrade's frames and a body to the close are not
    /// followed (no head read from them).
    #[test]
    fn responses_are_framed_as_their_heads_say() {
        let stream = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nabcHTTP/1.1 304 Not Modified\r\netag: x\r\n\r\nHTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\nx-t: 1\r\n\r\nHTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n";
        let st = |s| Head::Response { status: s };
        let want = vec![st(200), st(304), st(200), st(404)];
        for cut in 1..stream.len() {
            assert_eq!(heads(Side::Responses, &[&stream[..cut], &stream[cut..]], false), want, "cut at {cut}");
        }
        // a HEAD's answer names a length it does not carry
        assert_eq!(heads(Side::Responses, &[b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\nHTTP/1.1 200 OK\r\n\r\n"], true), vec![st(200), st(200)]);
        // a socket's frames after 101 are not heads
        assert_eq!(heads(Side::Responses, &[b"HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\n\r\nHTTP/1.1 200 OK\r\n\r\n"], false), vec![st(101)]);
        // a body to the close
        assert_eq!(heads(Side::Responses, &[b"HTTP/1.1 200 OK\r\n\r\nHTTP/1.1 500 x\r\n\r\n"], false), vec![st(200)]);
    }

    /// Invalid: what is no HTTP, a head past its limit, a chunk size that
    /// is no number: nothing more is read, and nothing panics.
    #[test]
    fn what_it_cannot_follow_ends_its_watching() {
        assert_eq!(heads(Side::Requests, &[b"\x16\x03\x01 not http\r\n\r\nGET / HTTP/1.1\r\n\r\n"], false), vec![]);
        let long = vec![b'a'; HEAD_MAX + 10];
        assert_eq!(heads(Side::Requests, &[b"GET /", &long, b" HTTP/1.1\r\n\r\n"], false), vec![]);
        assert_eq!(heads(Side::Requests, &[b"POST /x HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\nzz\r\nGET / HTTP/1.1\r\n\r\n"], false), vec![req("POST", "/x")]);
        assert_eq!(heads(Side::Requests, &[b"POST /x HTTP/1.1\r\ncontent-length: nope\r\n\r\n"], false), vec![]);
    }

    #[test]
    fn a_line_says_what_was_asked_and_answered() {
        assert_eq!(said("GET", "/__shell/shell.js", 304, Duration::from_millis(12)), "[front] GET /__shell/shell.js 304 (12ms)\n");
        assert_eq!(said("GET", &"a".repeat(300), 200, Duration::ZERO).len(), "[front] GET ".len() + 200 + " 200 (0ms)\n".len());
    }
}
