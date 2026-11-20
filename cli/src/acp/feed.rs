//! A chat as it happens: the records of its `chat` and `work` and the
//! drafts of its `chat`, from its live socket (docs/api.md, `__live`), as
//! the chat's page follows it. A draft is never stored, so only the socket
//! carries a reply while it is written.
//!
//! The socket is read a little at a time (`next`'s wait), so its reader
//! also hears a Stop and its client leaving. One that drops is dialed
//! again, more slowly each time, from the last record of each channel: a
//! record is never missed or told twice, and a draft lost meanwhile is
//! made good by the next, which is whole, or by its reply.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fragment_proto::live::{Cursor, LiveIn, LiveOut, Subscribe};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::chats::Seqs;
use super::turn::Record;
use crate::api::Client;

/// A socket idle this long is pinged, so nothing between here and the
/// fragment takes it for dead (a proxy's idle bound is about 100 s).
const PING_EVERY: Duration = Duration::from_secs(30);
/// The longest wait between dials of a socket that dropped: doubled from
/// one second, as the watcher's (watch.rs).
const REDIAL_MAX: Duration = Duration::from_secs(30);
/// A dial takes at most this long, all of it (a request's own connect
/// timeout, api.rs): a name that does not resolve or a host that stalls
/// holds no prompt past it, so a cancel and the client's leaving are
/// still heard.
const DIAL_TIMEOUT: Duration = crate::api::CONNECT_TIMEOUT;

/// What a chat's feed says next.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A record of `chat` or `work`, each channel's in order, once.
    Record(Record),
    /// A reply as it is written: its whole text so far, or none once its
    /// poster stopped.
    Draft { principal: String, turn: String, text: Option<String> },
    /// Nothing within the wait.
    Quiet,
}

/// A chat's records and drafts as they come.
pub trait Feed {
    /// The next event, waiting at most about `wait` for one.
    fn next(&mut self, wait: Duration) -> Event;
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// The chat's live socket.
pub struct Live<'a> {
    client: &'a Client,
    chat: String,
    /// The last record told of each channel: a dial subscribes after them.
    seen: Seqs,
    socket: Option<Socket>,
    /// When the socket last carried anything either way.
    heard: Instant,
    /// A dropped socket's next dial, and the wait after it.
    redial: (Instant, Duration),
    /// `DIAL_TIMEOUT`, or a test's.
    dial_timeout: Duration,
}

impl<'a> Live<'a> {
    /// Follows `chat` from after `seen`, dialed and subscribed before it
    /// answers: what the chat says from here on is told.
    pub fn open(client: &'a Client, chat: &str, seen: Seqs) -> Result<Live<'a>> {
        Live::open_within(client, chat, seen, DIAL_TIMEOUT)
    }

    fn open_within(client: &'a Client, chat: &str, seen: Seqs, dial_timeout: Duration) -> Result<Live<'a>> {
        let mut live = Live { client, chat: chat.to_string(), seen, socket: None, heard: Instant::now(), redial: (Instant::now(), Duration::from_secs(1)), dial_timeout };
        live.dial().with_context(|| format!("following {chat}'s live socket"))?;
        Ok(live)
    }

    /// Dials and subscribes, the whole of it within the dial's deadline
    /// (`connect`): tungstenite's own `connect` has none, and a dial that
    /// stalled would hold its prompt (and so the whole server's exit) for
    /// as long as it did.
    fn dial(&mut self) -> Result<()> {
        use tungstenite::client::IntoClientRequest;
        let http = self.client.url("GET", &format!("/f/{}/__live", self.chat));
        // signed fresh on every dial: a NIP-98 event is good for a minute
        let mut request = http.replacen("http", "ws", 1).into_client_request().context("the live socket's URL")?;
        let (header, value) = self.client.signer.header("GET", &http, &[]);
        request.headers_mut().insert(header, value.parse().context("the signature's header")?);
        let uri = request.uri().clone();
        let host = uri.host().context("the live socket's URL names a host")?.trim_start_matches('[').trim_end_matches(']').to_string();
        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
        self.socket = Some(connect(request, &host, port, self.dial_timeout, self.seen)?);
        self.heard = Instant::now();
        Ok(())
    }

    /// The socket dropped: dial again once its wait is over.
    fn dropped(&mut self) {
        self.socket = None;
        let wait = self.redial.1;
        self.redial = (Instant::now() + wait, (wait * 2).min(REDIAL_MAX));
    }

    /// What a frame says. A record at or before the last told is no news
    /// (a page read twice), and a frame this reader does not know is
    /// skipped (docs/wire: the socket's frames are open).
    fn told(&mut self, frame: &str) -> Event {
        match serde_json::from_str::<LiveOut>(frame) {
            Ok(LiveOut::Record(r)) => {
                let last = match r.channel.as_str() {
                    "chat" => &mut self.seen.chat,
                    "work" => &mut self.seen.work,
                    _ => return Event::Quiet,
                };
                if r.seq <= *last {
                    return Event::Quiet;
                }
                *last = r.seq;
                let body = serde_json::from_str(r.body.get()).unwrap_or_default();
                Event::Record(Record { channel: r.channel, seq: r.seq, at: r.at, principal: r.principal, body })
            }
            Ok(LiveOut::Draft { channel, principal, turn, text, .. }) if channel == "chat" => Event::Draft { principal, turn, text },
            // a page that ended before the channel's end: the next one
            Ok(LiveOut::Subscribed { channel, next, more: true }) => {
                let sent = self.socket.as_mut().map(|s| s.send(Message::Text(subscribe(&channel, next).into())));
                if !matches!(sent, Some(Ok(()))) {
                    self.dropped();
                }
                Event::Quiet
            }
            _ => Event::Quiet,
        }
    }
}

/// One dial, all of it within `wait`: the name resolved, each address
/// tried in turn, the socket's handshake, and the two subscribes. Nothing
/// of a dial given up on outlives its deadline but a resolver's thread,
/// which holds no socket.
///
/// - Resolving a name has no timeout to set, so it is asked on a thread of
///   its own and waited for.
/// - The handshake's reads each wait their own timeout, so a host that
///   trickles its answer would outlast every one of them: at the deadline
///   the socket itself is shut (a second handle to it, on a thread that
///   sleeps till then), which ends whatever read is in flight and closes
///   it for the host too.
fn connect(request: tungstenite::handshake::client::Request, host: &str, port: u16, wait: Duration, seen: Seqs) -> Result<Socket> {
    use std::net::ToSocketAddrs;
    use std::sync::atomic::{AtomicBool, Ordering};
    let deadline = Instant::now() + wait;
    let left = || deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
    let late = || anyhow::anyhow!("{host}:{port} did not open the live socket within {wait:?}");

    let (done, resolved) = std::sync::mpsc::channel();
    let name = (host.to_string(), port);
    std::thread::spawn(move || {
        let _ = done.send(name.to_socket_addrs().map(Iterator::collect::<Vec<_>>));
    });
    let addresses = resolved.recv_timeout(wait).map_err(|_| late())?.with_context(|| format!("resolving {host}"))?;
    let stream = addresses.iter().find_map(|address| TcpStream::connect_timeout(address, left()).ok()).ok_or_else(late)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(left()))?;
    stream.set_write_timeout(Some(left()))?;

    let (watched, open) = (stream.try_clone()?, std::sync::Arc::new(AtomicBool::new(false)));
    let opened = std::sync::Arc::clone(&open);
    std::thread::spawn(move || {
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        if !opened.load(Ordering::Relaxed) {
            let _ = watched.shutdown(std::net::Shutdown::Both);
        }
    });
    let shaken = tungstenite::client_tls(request, stream).map_err(|e| match e {
        // past the deadline, whatever it failed with, it was late
        _ if Instant::now() >= deadline => late(),
        tungstenite::handshake::HandshakeError::Failure(why) => anyhow::Error::new(why),
        tungstenite::handshake::HandshakeError::Interrupted(_) => late(),
    });
    let (mut socket, _) = shaken?;
    for (channel, after) in [("work", seen.work), ("chat", seen.chat)] {
        socket.send(Message::Text(subscribe(channel, after).into()))?;
    }
    open.store(true, Ordering::Relaxed);
    // what is sent from here on (a ping, a page's next subscribe) waits a
    // dial's time, not what was left of this one's
    let _ = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_write_timeout(Some(wait)),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_write_timeout(Some(wait)),
        _ => Ok(()),
    };
    Ok(socket)
}

fn subscribe(channel: &str, after: i64) -> String {
    LiveIn::Subscribe(Subscribe { channel: channel.to_string(), from: Cursor::After(after) }).encode()
}

/// A read of `socket` waits at most `wait`: its stream's own timeout,
/// under TLS or none.
fn wait_at_most(socket: &mut Socket, wait: Duration) {
    let wait = Some(wait.max(Duration::from_millis(1)));
    let _ = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(wait),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_read_timeout(wait),
        _ => Ok(()),
    };
}

impl Feed for Live<'_> {
    fn next(&mut self, wait: Duration) -> Event {
        let Some(socket) = self.socket.as_mut() else {
            let left = self.redial.0.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                std::thread::sleep(left.min(wait));
            } else if self.dial().is_ok() {
                self.redial.1 = Duration::from_secs(1);
            } else {
                self.dropped();
            }
            return Event::Quiet;
        };
        wait_at_most(socket, wait);
        match socket.read() {
            Ok(Message::Text(frame)) => {
                self.heard = Instant::now();
                self.told(&frame)
            }
            Ok(Message::Close(_)) => {
                self.dropped();
                Event::Quiet
            }
            Ok(_) => Event::Quiet,
            // nothing within the wait: the socket is as it was
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if self.heard.elapsed() >= PING_EVERY {
                    self.heard = Instant::now();
                    if socket.send(Message::Text(LiveIn::Ping.encode().into())).is_err() {
                        self.dropped();
                    }
                }
                Event::Quiet
            }
            Err(_) => {
                self.dropped();
                Event::Quiet
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a host that takes the connection and never answers the
    /// socket's handshake holds a dial only for its deadline (the whole
    /// dial's, whichever step stalls), so a prompt still hears its cancel
    /// and its client leaving.
    #[test]
    fn a_stalled_handshake_ends_at_its_deadline() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
        let port = listener.local_addr().expect("its address").port();
        // accepts, reads nothing, says nothing
        let held = std::thread::spawn(move || {
            let taken = listener.accept();
            std::thread::sleep(Duration::from_secs(3));
            drop(taken);
        });
        let client = Client::new(&format!("http://127.0.0.1:{port}"), crate::auth::fixed(7));
        let began = Instant::now();
        let refused = Live::open_within(&client, "c--k3x9", Seqs { chat: 0, work: 0 }, Duration::from_millis(200)).err().expect("no socket opens");
        assert!(began.elapsed() < Duration::from_secs(2), "it waited {:?}", began.elapsed());
        assert!(format!("{refused:#}").contains("within 200ms"), "{refused:#}");
        let _ = held.join();
    }

    /// Goal: a dial given up on leaves nothing behind. A host that
    /// trickles its handshake, each byte inside any one read's timeout,
    /// has its socket shut at the dial's deadline, not held for as long as
    /// it goes on trickling.
    #[test]
    fn a_trickled_handshake_is_shut_at_the_deadline() {
        use std::io::Write as _;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
        let port = listener.local_addr().expect("its address").port();
        // an answer that never ends, a byte of it every 100 ms for five
        // seconds or until the dialer is gone: when that was
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let began = Instant::now();
            let mut answer = b"HTTP/1.1 101 Switching Protocols\r\nx-pad: ".iter().copied().chain(std::iter::repeat(b'a'));
            let mut gone = None;
            while began.elapsed() < Duration::from_secs(5) {
                let byte = answer.next().expect("it never ends");
                if stream.write_all(&[byte]).is_err() {
                    gone = Some(began.elapsed());
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = said.send(gone);
        });
        let client = Client::new(&format!("http://127.0.0.1:{port}"), crate::auth::fixed(7));
        let began = Instant::now();
        assert!(Live::open_within(&client, "c--k3x9", Seqs { chat: 0, work: 0 }, Duration::from_millis(300)).is_err(), "no socket opens");
        assert!(began.elapsed() < Duration::from_secs(2), "it waited {:?}", began.elapsed());
        let gone = heard.recv_timeout(Duration::from_secs(8)).expect("the host says when its dialer left");
        assert!(gone.is_some_and(|after| after < Duration::from_secs(2)), "the dial's socket was shut at its deadline, not left open: {gone:?}");
    }
}
