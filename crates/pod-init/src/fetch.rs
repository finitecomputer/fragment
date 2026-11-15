//! Just enough of an HTTP/1.1 client to ask a key service: one request,
//! `Connection: close`, the answer read to its end, bounded in size and
//! time. pod-init has no HTTP crate (Cargo.toml), and the key service
//! speaks this crate's own server (`http`), which always closes after one answer.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// A delivery is under 1 KiB and a refusal a few hundred bytes.
pub const MAX_RESPONSE: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The name resolves to nothing, or the connection failed.
    Connect(String),
    Io(String),
    TooLarge,
    Malformed(&'static str),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "connecting: {e}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::TooLarge => write!(f, "answer over {MAX_RESPONSE} bytes"),
            Self::Malformed(why) => write!(f, "malformed answer: {why}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// `addr` is host:port, already checked (config::check_host_port).
pub fn request(addr: &str, method: &str, path: &str, body: &[u8]) -> Result<(u16, Vec<u8>), FetchError> {
    let mut stream = connect(addr)?;
    let io = |e: std::io::Error| FetchError::Io(e.to_string());
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(io)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(io)?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)).map_err(io)?;
    let mut answer = Vec::new();
    stream.take(MAX_RESPONSE as u64 + 1).read_to_end(&mut answer).map_err(io)?;
    if answer.len() > MAX_RESPONSE {
        return Err(FetchError::TooLarge);
    }
    parse(&answer)
}

fn connect(addr: &str) -> Result<TcpStream, FetchError> {
    let mut last = String::from("no address");
    for a in addr.to_socket_addrs().map_err(|e| FetchError::Connect(e.to_string()))? {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("{a}: {e}"),
        }
    }
    Err(FetchError::Connect(last))
}

/// The status and the body; the body is cut to Content-Length if one
/// is given, and is what remains otherwise.
pub fn parse(answer: &[u8]) -> Result<(u16, Vec<u8>), FetchError> {
    let end = answer.windows(4).position(|w| w == b"\r\n\r\n").ok_or(FetchError::Malformed("no end of head"))?;
    let head = std::str::from_utf8(&answer[..end]).map_err(|_| FetchError::Malformed("head is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return Err(FetchError::Malformed("status line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(FetchError::Malformed("not HTTP/1.x"));
    }
    let status = code.parse().map_err(|_| FetchError::Malformed("status code"))?;
    let mut body = answer[end + 4..].to_vec();
    let length = lines.filter_map(|l| l.split_once(':')).find(|(n, _)| n.eq_ignore_ascii_case("content-length"));
    if let Some((_, v)) = length {
        let n: usize = v.trim().parse().map_err(|_| FetchError::Malformed("Content-Length"))?;
        if n > body.len() {
            return Err(FetchError::Malformed("body shorter than Content-Length"));
        }
        body.truncate(n);
    }
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Response;

    #[test]
    fn reads_what_the_http_server_writes() {
        let mut out = Vec::new();
        Response::json(403, b"{\"error\":\"x\"}".to_vec()).write_to(&mut out).unwrap();
        assert_eq!(parse(&out).unwrap(), (403, b"{\"error\":\"x\"}".to_vec()));
    }

    #[test]
    fn refuses_what_is_not_an_answer() {
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\n"), Err(FetchError::Malformed("no end of head")));
        assert_eq!(parse(b"SPDY 200 OK\r\n\r\n"), Err(FetchError::Malformed("not HTTP/1.x")));
        assert_eq!(parse(b"HTTP/1.1 OK\r\n\r\n"), Err(FetchError::Malformed("status code")));
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nab").is_err());
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nab").unwrap().1, b"a");
    }

    #[test]
    fn an_address_nobody_listens_on_is_a_connect_error() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(matches!(request(&free.to_string(), "GET", "/nonce", b""), Err(FetchError::Connect(_))));
    }
}
