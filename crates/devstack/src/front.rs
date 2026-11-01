//! The node's front door: the port its clients use (the e2e's, a browser's,
//! the CLI's, the cell's own calls to its platform URL), each connection
//! passed byte for byte to the runtime that runs the Worker, past `wrangler
//! dev`'s own proxy.
//!
//! wrangler answers on its port through a proxy of its own (its
//! ProxyWorker, a Durable Object in a second workerd), which sends each
//! request on to the Worker's runtime over connections it pools. That
//! runtime closes a connection idle 5 s, and a request the proxy writes onto
//! one as it closes is lost: the proxy asks a GET or HEAD again, and answers
//! anything else 500 "Network connection lost" (a test's PUT in CI run
//! 40341674686, a CLI approval's POST in 40404853530; tests/front_door.rs
//! makes it happen at will). Pages' module requests went unanswered behind
//! it too, under load (the chat's fit, a shell reloaded: CI runs
//! 41086455263, 41092418386), which a Worker of a few lines reproduced
//! there and never past it. So the node's port is ours: each connection a
//! client opens is one connection to the runtime, nothing between them that
//! reads a request, and no pool.
//!
//! Each request and its answer is a line of the node's log (`[front] GET
//! /x 200 (3ms)`, framing.rs), as wrangler's proxy wrote one: wrangler
//! writes none of the runtime's own. A request a connection closed on
//! unanswered says so.
//!
//! wrangler names the runtime's address only in its debug output, where its
//! proxy is told of each start and restart (`reloadStart`, then
//! `reloadComplete` with the address; a restart moves the port): the node's
//! log is read for it as wrangler writes it (`heard`), and a connection
//! opened during a restart waits for the next address, as wrangler's proxy
//! holds a request (docs/technical-debt-ledger.md).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::framing::{self, Framer, Head, Side};

/// What wrangler's debug output tells its proxy as the runtime restarts.
const RESTARTING: &str = r#""type":"reloadStart""#;
/// … and once it runs, with its address.
const RESTARTED: &str = r#""type":"reloadComplete""#;

/// The longest a connection waits for the runtime: through a restart (a
/// rebuild's, the e2e's reload), as wrangler's proxy would hold its
/// request. Past it, it is closed.
const RUNTIME_WAIT: Duration = Duration::from_secs(90);

/// The runtime's address as wrangler last told its proxy (`port`), whether
/// it is restarting, and whether wrangler is gone (its output ended).
#[derive(Default)]
struct Runtime {
    port: Option<u16>,
    restarting: bool,
    gone: bool,
}

/// The front door's shared state: the runtime, and its changes.
#[derive(Default)]
pub(crate) struct Heard {
    runtime: Mutex<Runtime>,
    changed: Condvar,
}

impl Heard {
    /// One line of wrangler's output.
    pub(crate) fn line(&self, line: &[u8]) {
        let line = String::from_utf8_lossy(line);
        let mut r = self.runtime.lock().unwrap_or_else(PoisonError::into_inner);
        if line.contains(RESTARTING) {
            r.restarting = true;
        } else if line.contains(RESTARTED) {
            // a restart that names no runtime leaves the last one named:
            // `named` fails a start that never heard one
            if let Some(port) = runtime_port(&line) {
                r.port = Some(port);
                r.restarting = false;
            }
        } else {
            return;
        }
        self.changed.notify_all();
    }

    /// wrangler's output ended: it is gone.
    pub(crate) fn ended(&self) {
        self.runtime.lock().unwrap_or_else(PoisonError::into_inner).gone = true;
        self.changed.notify_all();
    }

    /// The runtime's port once wrangler has named one, waiting at most
    /// `wait`: `None` when it did not, or wrangler is gone.
    pub(crate) fn named(&self, wait: Duration) -> Option<u16> {
        self.running(None, wait)
    }

    /// The runtime's port once it runs, and is not `not` (a port that
    /// refused a connection: the runtime restarted from it); `None` when it
    /// is gone, or `wait` passed.
    fn running(&self, not: Option<u16>, wait: Duration) -> Option<u16> {
        let t0 = Instant::now();
        let mut r = self.runtime.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if r.gone {
                return None;
            }
            match r.port {
                Some(port) if !r.restarting && Some(port) != not => return Some(port),
                _ => {}
            }
            let left = wait.checked_sub(t0.elapsed())?;
            r = self.changed.wait_timeout(r, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

/// The runtime's port in a `reloadComplete` line of wrangler's debug
/// output: its `userWorkerUrl` on 127.0.0.1, as wrangler runs it.
fn runtime_port(line: &str) -> Option<u16> {
    let url = &line[line.find(r#""userWorkerUrl":{"#)?..];
    let url = &url[..url.find('}')?];
    if !url.contains(r#""hostname":"127.0.0.1""#) {
        return None;
    }
    let port = &url[url.find(r#""port":""#)? + r#""port":""#.len()..];
    port[..port.find('"')?].parse().ok().filter(|p| *p != 0)
}

/// The node's port, listened on from before wrangler starts until the node
/// is dropped: connections accepted on it are passed to the runtime.
pub(crate) struct Front {
    port: u16,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl Front {
    /// Passes what `listener` accepts (the node's port, bound by its maker:
    /// `NodeOptions::listener`) to the runtime `heard` names, each request a
    /// line of `log` (the boot's).
    pub(crate) fn serve(listener: TcpListener, heard: Arc<Heard>, log: Arc<Mutex<File>>) -> Result<Front> {
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let accepting = thread::Builder::new().name(format!("front-{port}")).spawn(move || {
            for client in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(client) = client else { continue };
                let (heard, log) = (heard.clone(), log.clone());
                // a thread a connection, as long as it is open
                let _ = thread::Builder::new().name(format!("front-{port}-pass")).spawn(move || pass(client, &heard, &log));
            }
        })?;
        Ok(Front { port, stop, accepting: Some(accepting) })
    }
}

impl Drop for Front {
    /// Its listener let go (the port stays its maker's, for the node's next
    /// life). Connections still open end with the runtime's.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // the accept loop wakes at a connection, sees the stop, and returns
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.accepting.take() {
            let _ = t.join();
        }
    }
}

/// One client's connection, passed to the runtime both ways until either
/// side ends it, each request and its answer a line of `log`.
fn pass(client: TcpStream, heard: &Heard, log: &Mutex<File>) {
    let mut refused = None;
    let runtime = loop {
        let Some(port) = heard.running(refused, RUNTIME_WAIT) else { return };
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            // restarted from that port before wrangler said so: the next
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => refused = Some(port),
            Err(_) => return,
        }
    };
    // small writes (a socket's frames, a response's head) go at once
    let _ = client.set_nodelay(true);
    let _ = runtime.set_nodelay(true);
    let (Ok(client_in), Ok(runtime_in), Ok(runtime_end)) = (client.try_clone(), runtime.try_clone(), runtime.try_clone()) else { return };
    // the requests asked and not yet answered, oldest first: an HTTP/1.1
    // connection answers them in order
    let asked: Arc<Mutex<VecDeque<(String, String, Instant)>>> = Arc::default();
    let up = {
        let asked = asked.clone();
        let mut requests = Framer::new(Side::Requests);
        thread::Builder::new().name("front-up".into()).spawn(move || {
            copy(client_in, runtime, |bytes| {
                requests.feed(bytes, &|| false, |head| {
                    if let Head::Request { method, path } = head {
                        asked.lock().unwrap_or_else(PoisonError::into_inner).push_back((method, path, Instant::now()));
                    }
                })
            });
            // the client's end done (closed, or failed), the runtime's is
            // closed whole, as a proxy does once its client goes: workerd
            // keeps a connection it is only half-closed on, and so would
            // this thread waiting on it (a reload's dropped streams and
            // sockets, by the thousand). A client that half-closes and waits
            // for its answer gets none; no client here does.
            let _ = runtime_end.shutdown(Shutdown::Both);
        })
    };
    let mut responses = Framer::new(Side::Responses);
    copy(runtime_in, client, |bytes| {
        let head_request = || asked.lock().unwrap_or_else(PoisonError::into_inner).front().is_some_and(|(m, ..)| m == "HEAD");
        responses.feed(bytes, &head_request, |head| {
            // an interim answer (100, 103) is not the request's
            let Head::Response { status } = head else { return };
            if (100..200).contains(&status) && status != 101 {
                return;
            }
            if let Some((method, path, at)) = asked.lock().unwrap_or_else(PoisonError::into_inner).pop_front() {
                write(log, &framing::said(&method, &path, status, at.elapsed()));
            }
        })
    });
    if let Ok(up) = up {
        let _ = up.join();
    }
    for (method, path, at) in asked.lock().unwrap_or_else(PoisonError::into_inner).drain(..) {
        let path: String = path.chars().take(200).collect();
        write(log, &format!("[front] {method} {path} unanswered: its connection closed after {}ms\n", at.elapsed().as_millis()));
    }
}

/// A line into the boot's log, beside wrangler's own.
fn write(log: &Mutex<File>, line: &str) {
    let _ = log.lock().unwrap_or_else(PoisonError::into_inner).write_all(line.as_bytes());
}

/// `from`'s bytes to `to` until `from` ends (or either fails), each read
/// `seen` before it is passed on, then `to`'s writing side shut, as
/// `from`'s was.
fn copy(mut from: TcpStream, mut to: TcpStream, mut seen: impl FnMut(&[u8])) {
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // seen first: a request is recorded before the runtime can answer
        // it, and one that never went on is still said, unanswered
        seen(&buf[..n]);
        if to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.flush();
    let _ = to.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const NAMED: &str = r#"[InspectorProxyWorker] handleProxyControllerIncomingMessage {"type":"reloadComplete","proxyData":{"userWorkerUrl":{"protocol":"http:","hostname":"127.0.0.1","port":"43765"},"userWorkerInspectorUrl":{"protocol":"ws:","hostname":"127.0.0.1","port":"43153","pathname":"/core:user:fragment"},"headers":{"MF-Proxy-Shared-Secret":"05346fd5"},"liveReload":false}}"#;

    /// Valid: a `reloadComplete` line names the runtime's port. Invalid: a
    /// line naming none, another host, port 0, or junk names nothing.
    #[test]
    fn the_runtime_is_named_by_wranglers_restart_line() {
        assert_eq!(runtime_port(NAMED), Some(43765));
        assert_eq!(runtime_port(&NAMED.replace("127.0.0.1\",\"port\":\"43765", "0.0.0.0\",\"port\":\"43765")), None);
        assert_eq!(runtime_port(&NAMED.replace("\"port\":\"43765\"", "\"port\":\"0\"")), None);
        assert_eq!(runtime_port(&NAMED.replace("\"port\":\"43765\"", "\"port\":\"x\"")), None);
        assert_eq!(runtime_port(r#"{"type":"reloadComplete","proxyData":{}}"#), None);
        assert_eq!(runtime_port("[wrangler:info] Ready on http://127.0.0.1:8790"), None);
    }

    /// A start, a restart that moves the port, and wrangler's end: a
    /// connection waits through the restart and meets the new port; once
    /// wrangler is gone, none waits.
    #[test]
    fn a_connection_waits_through_a_restart() {
        let heard = Heard::default();
        assert_eq!(heard.named(Duration::from_millis(10)), None, "nothing named yet");
        heard.line(NAMED.as_bytes());
        assert_eq!(heard.named(Duration::ZERO), Some(43765));
        heard.line(br#"[InspectorProxyWorker] handleProxyControllerIncomingMessage {"type":"reloadStart"}"#);
        assert_eq!(heard.running(None, Duration::from_millis(10)), None, "restarting: no port");
        heard.line(NAMED.replace("43765", "39825").as_bytes());
        assert_eq!(heard.running(None, Duration::ZERO), Some(39825));
        // replay: the same line again changes nothing
        heard.line(NAMED.replace("43765", "39825").as_bytes());
        assert_eq!(heard.running(Some(43765), Duration::ZERO), Some(39825));
        assert_eq!(heard.running(Some(39825), Duration::from_millis(10)), None, "refused there, and nothing newer");
        heard.ended();
        assert_eq!(heard.running(None, Duration::from_secs(5)), None, "gone: no wait");
    }

    /// Two ends of one connection on this machine.
    fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let near = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        (near, l.accept().unwrap().0)
    }

    /// What `s` has for us now, without waiting: how many bytes.
    fn drain(s: &mut TcpStream) -> usize {
        let (mut buf, mut got) = ([0; 64 * 1024], 0);
        loop {
            match s.read(&mut buf) {
                Ok(0) => return got,
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return got,
                Err(e) => panic!("the far end reads: {e}"),
            }
        }
    }

    /// Each read is seen before it is passed on: whenever `seen` runs, the
    /// far end has none of that read. So a request is recorded before the
    /// runtime can answer it (Alex's review of #748: a fast answer to a
    /// request not yet recorded was dropped, the next answers each said as
    /// the request before's). Bytes passed on before may still be on their
    /// way (the sender holds small writes while an ACK is out: CI's runners
    /// held 36 KB once), so only bytes past those seen are early.
    #[test]
    fn a_read_is_seen_before_it_is_passed_on() {
        let (mut client, from) = pair();
        let (to, mut far) = pair();
        far.set_nonblocking(true).unwrap();
        let copying = thread::spawn(move || {
            let (mut passed, mut arrived, mut early) = (0, 0, Vec::new());
            copy(from, to, |bytes| {
                arrived += drain(&mut far);
                if arrived > passed {
                    early.push((passed, arrived));
                }
                passed += bytes.len();
            });
            far.set_nonblocking(false).unwrap();
            let mut rest = Vec::new();
            far.read_to_end(&mut rest).unwrap();
            (passed, arrived + rest.len(), early)
        });
        let sent: usize = (0..500).map(|i| client.write(format!("GET /{i} HTTP/1.1\r\nhost: x\r\n\r\n").repeat(1 + i % 7).as_bytes()).unwrap()).sum();
        client.shutdown(Shutdown::Write).unwrap();
        let (passed, arrived, early) = copying.join().unwrap();
        assert!(early.is_empty(), "bytes reached the far end before they were seen (passed, arrived): {early:?}");
        assert_eq!((passed, arrived), (sent, sent), "every byte passed on");
    }

    /// A runtime of a few lines: each request answered at once, its status
    /// the path's last part; a 200's or 404's body is 5 bytes (none to a
    /// HEAD, though its length says 5), a 204's or 304's none.
    fn serve(s: TcpStream) {
        let mut answer = s.try_clone().unwrap();
        let mut asked = io::BufReader::new(s);
        loop {
            let mut line = String::new();
            if io::BufRead::read_line(&mut asked, &mut line).unwrap_or(0) == 0 {
                return;
            }
            let mut header = String::new();
            while io::BufRead::read_line(&mut asked, &mut header).unwrap_or(0) > 2 {
                header.clear();
            }
            let mut parts = line.split_whitespace();
            let (method, path) = (parts.next().unwrap(), parts.next().unwrap());
            let status: u16 = path.rsplit('/').next().unwrap().parse().unwrap();
            let said = match status {
                204 | 304 => format!("HTTP/1.1 {status} X\r\n\r\n"),
                _ if method == "HEAD" => format!("HTTP/1.1 {status} X\r\ncontent-length: 5\r\n\r\n"),
                _ => format!("HTTP/1.1 {status} X\r\ncontent-length: 5\r\n\r\nhello"),
            };
            if answer.write_all(said.as_bytes()).is_err() {
                return;
            }
        }
    }

    /// An answer read whole, as a client reads it: its status.
    fn answer(r: &mut io::BufReader<TcpStream>, method: &str) -> u16 {
        let mut line = String::new();
        io::BufRead::read_line(r, &mut line).unwrap();
        let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut length = 0;
        loop {
            let mut h = String::new();
            io::BufRead::read_line(r, &mut h).unwrap();
            if h == "\r\n" {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        if method != "HEAD" && !matches!(status, 204 | 304) {
            r.read_exact(&mut vec![0; length]).unwrap();
        }
        status
    }

    /// Goal: the node's port is its maker's across the node's lives. With a
    /// life's front door gone, no other socket can bind the port, and a
    /// client that dials then is answered once the next life's door is
    /// open, as one dialing during a boot is.
    #[test]
    fn a_held_port_outlives_each_front_door() {
        let runtime = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let heard = Arc::new(Heard::default());
        heard.line(NAMED.replace("43765", &runtime.local_addr().unwrap().port().to_string()).as_bytes());
        // bounded by the test's process: it ends with it
        thread::spawn(move || {
            for s in runtime.incoming().flatten() {
                thread::spawn(move || serve(s));
            }
        });
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let path = std::env::temp_dir().join(format!("front-log-{}", crate::random_hex(4)));
        let log = Arc::new(Mutex::new(File::create(&path).unwrap()));
        let asked = || {
            let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
            client.write_all(b"GET /a/200 HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
            io::BufReader::new(client)
        };
        let first = Front::serve(held.try_clone().unwrap(), heard.clone(), log.clone()).unwrap();
        assert_eq!(answer(&mut asked(), "GET"), 200, "through the first life's door");
        drop(first);
        assert!(TcpListener::bind(("127.0.0.1", port)).is_err(), "no other socket binds the port between lives");
        let mut waiting = asked();
        let _second = Front::serve(held.try_clone().unwrap(), heard, log).unwrap();
        assert_eq!(answer(&mut waiting, "GET"), 200, "a client that dialed between lives, answered through the next");
        std::fs::remove_file(&path).ok();
    }

    /// Through `pass`, to a runtime that answers each request at once: 1,000
    /// requests kept alive on one connection, each with a 4 KiB header, HEAD
    /// then GET in turn, their paths and statuses mixed. Every answer
    /// reaches the client, and the log says each request with its own
    /// answer, in order, none unanswered (Alex's review of #748: one in a
    /// thousand was dropped and another said unanswered).
    #[test]
    fn each_request_is_logged_with_its_own_answer() {
        const ASKED: usize = 1_000;
        const STATUSES: [u16; 4] = [200, 204, 304, 404];
        let runtime = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let heard = Heard::default();
        heard.line(NAMED.replace("43765", &runtime.local_addr().unwrap().port().to_string()).as_bytes());
        let serving = thread::spawn(move || serve(runtime.accept().unwrap().0));
        let front = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let at = front.local_addr().unwrap();
        let path = std::env::temp_dir().join(format!("front-log-{}", crate::random_hex(4)));
        let log = Arc::new(Mutex::new(File::create(&path).unwrap()));
        let passing = thread::spawn(move || pass(front.accept().unwrap().0, &heard, &log));
        let mut client = TcpStream::connect(at).unwrap();
        let mut reader = io::BufReader::new(client.try_clone().unwrap());
        let cookie = "c".repeat(4096);
        let mut expected = Vec::new();
        for i in 0..ASKED {
            let (method, status) = (if i % 2 == 0 { "HEAD" } else { "GET" }, STATUSES[i / 2 % STATUSES.len()]);
            let path = format!("/r{i}/{status}");
            client.write_all(format!("{method} {path} HTTP/1.1\r\nhost: x\r\ncookie: {cookie}\r\n\r\n").as_bytes()).unwrap();
            assert_eq!(answer(&mut reader, method), status, "{method} {path}'s answer reaches the client");
            expected.push(format!("[front] {method} {path} {status}"));
        }
        drop((client, reader));
        passing.join().unwrap();
        serving.join().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        let said: Vec<&str> = text.lines().map(|l| l.rsplit_once(" (").map_or(l, |(said, _)| said)).collect();
        let first_wrong = said.iter().zip(&expected).position(|(s, e)| s != e);
        assert!(said.len() == ASKED && first_wrong.is_none(), "{} lines for {ASKED} requests; the first wrong: {:?}", said.len(), first_wrong.map(|i| (&expected[i], said[i])));
    }

    /// A client that goes, from a runtime that keeps the connection open and
    /// silent after it (workerd does so for a WebSocket whose client closed,
    /// half-closed or not): the pass ends, and lets the connection go. Before,
    /// each such pass waited on the runtime for good, a thread and two sockets
    /// a closed socket: a repro of reloads, each closing two WebSockets, held
    /// 28,000 in ten minutes, until connections were reset.
    #[test]
    fn a_client_that_goes_ends_its_pass_though_the_runtime_stays() {
        use std::sync::mpsc;
        let runtime = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let heard = Heard::default();
        heard.line(NAMED.replace("43765", &runtime.local_addr().unwrap().port().to_string()).as_bytes());
        let (stay, done) = mpsc::channel::<()>();
        thread::spawn(move || {
            let (mut s, _) = runtime.accept().unwrap();
            let mut buf = [0; 1024];
            let n = s.read(&mut buf).unwrap();
            assert!(buf[..n].ends_with(b"\r\n\r\n"), "a request");
            s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\r\n").unwrap();
            // the client's end seen, the connection kept, and nothing said
            while s.read(&mut buf).is_ok_and(|n| n > 0) {}
            let _ = done.recv();
        });
        let front = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let at = front.local_addr().unwrap();
        let (passed_tx, passed) = mpsc::channel();
        let path = std::env::temp_dir().join(format!("front-log-{}", crate::random_hex(4)));
        let log = Arc::new(Mutex::new(File::create(&path).unwrap()));
        thread::spawn(move || {
            pass(front.accept().unwrap().0, &heard, &log);
            passed_tx.send(()).unwrap();
        });
        let mut client = TcpStream::connect(at).unwrap();
        client.write_all(b"GET /ws HTTP/1.1\r\nhost: x\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\r\n").unwrap();
        let mut answer = [0; 12];
        client.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"HTTP/1.1 101");
        drop(client);
        // bounded only so a failure says so rather than hanging the suite
        assert_eq!(passed.recv_timeout(Duration::from_secs(10)), Ok(()), "the pass ended");
        drop(stay);
        std::fs::remove_file(&path).ok();
    }
}
