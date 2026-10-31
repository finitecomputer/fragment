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
//! wrangler names the runtime's address only in its debug output, where its
//! proxy is told of each start and restart (`reloadStart`, then
//! `reloadComplete` with the address; a restart moves the port): the node's
//! log is read for it as wrangler writes it (`heard`), and a connection
//! opened during a restart waits for the next address, as wrangler's proxy
//! holds a request (docs/technical-debt-ledger.md).

use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

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

/// The node's port, held from before wrangler starts until the node is
/// dropped: connections accepted on it are passed to the runtime.
pub(crate) struct Front {
    port: u16,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl Front {
    /// Holds 127.0.0.1:`port`, passing what it accepts to the runtime
    /// `heard` names.
    pub(crate) fn bind(port: u16, heard: Arc<Heard>) -> Result<Front> {
        let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("the node's port :{port} is taken"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let accepting = thread::Builder::new().name(format!("front-{port}")).spawn(move || {
            for client in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(client) = client else { continue };
                let heard = heard.clone();
                // a thread a connection, as long as it is open
                let _ = thread::Builder::new().name(format!("front-{port}-pass")).spawn(move || pass(client, &heard));
            }
        })?;
        Ok(Front { port, stop, accepting: Some(accepting) })
    }
}

impl Drop for Front {
    /// The port let go: the next node may hold it. Connections still open
    /// end with the runtime's.
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
/// side ends it.
fn pass(client: TcpStream, heard: &Heard) {
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
    let (Ok(client_in), Ok(runtime_in)) = (client.try_clone(), runtime.try_clone()) else { return };
    let up = thread::Builder::new().name("front-up".into()).spawn(move || copy(client_in, runtime));
    copy(runtime_in, client);
    if let Ok(up) = up {
        let _ = up.join();
    }
}

/// `from`'s bytes to `to` until `from` ends (or either fails), then `to`'s
/// writing side shut, as `from`'s was.
fn copy(mut from: TcpStream, mut to: TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.flush();
    let _ = to.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
