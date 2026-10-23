//! The platform's hosts on the guest's own loopback, where the platform's
//! zone is under `.localhost` (docs/computers.md, "The platform's hosts";
//! docs/self-host.md, found 40). Chrome and curl take a `.localhost` name
//! for the machine's own loopback without asking DNS (RFC 6761), so on a
//! local stack an app's address (`http://<app>.fragment.localhost:8790/`)
//! went to the guest's own 127.0.0.1 and never left it: the computer's
//! intercepts catch a name its resolver answered, by the address it
//! answered, on port 80 (HTTP's).
//!
//! So the bridge listens there, on the platform's port (127.0.0.1, and ::1
//! where the guest has it). A connection's first request names its host
//! (`Host`; plain HTTP has no other). A name in the zone is dialed by name
//! on port 80, through the guest's resolver, so the intercept catches it,
//! and the bytes go both ways as they are, the request's own `Host`
//! included (a WebSocket's too). Any other host is refused there (421),
//! reaching nothing. Every request after the first on that connection goes
//! where the first did: a browser keeps a connection to one host.

use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};

use crate::limits;

/// The port a name is dialed on: an HTTP intercept catches a name on port
/// 80 alone (Cloudflare's, and sandcastle's).
pub const DIAL_PORT: u16 = 80;

/// Where the platform's apps are: its zone and its port, from the
/// computer's own origin in the guest's view (`http://<label>.<zone>[:port]`,
/// on the platform's scheme and port, under its zone: cell config.rs,
/// `computer_origin`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Apps {
    pub zone: String,
    pub port: u16,
}

impl Apps {
    /// The apps to forward for a computer whose origin is `origin`: none
    /// unless it is plain HTTP under `.localhost` (elsewhere a name is
    /// asked of DNS, and the intercepts catch it as it is).
    pub fn of_origin(origin: &str) -> Option<Apps> {
        let authority = origin.strip_prefix("http://")?.split('/').next()?;
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok()?),
            None => (authority, 80),
        };
        let host = host.to_ascii_lowercase();
        let (_, zone) = host.split_once('.')?;
        let local = zone == "localhost" || zone.ends_with(".localhost");
        (local && port != 0 && name_ok(zone)).then(|| Apps { zone: zone.to_string(), port })
    }

    /// The name a request's `Host` names, when it is the zone's own or one
    /// under it, on this port: what the forwarder dials.
    pub fn name_of(&self, host: &str) -> Option<String> {
        let host = host.trim().to_ascii_lowercase();
        let (name, port) = match host.rsplit_once(':') {
            Some((n, p)) => (n, p.parse::<u16>().ok()?),
            None => (host.as_str(), 80),
        };
        let name = name.strip_suffix('.').unwrap_or(name);
        let under = name.strip_suffix(self.zone.as_str()).is_some_and(|label| label.len() > 1 && label.ends_with('.'));
        ((name == self.zone || under) && port == self.port && name_ok(name)).then(|| name.to_string())
    }
}

/// A host name: at most 253 bytes of labels of letters, digits and `-`.
fn name_ok(name: &str) -> bool {
    name.len() <= 253 && name.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// What a connection's first bytes say.
#[derive(Debug, PartialEq, Eq)]
enum Head<'a> {
    /// Not a whole request head yet.
    More,
    /// A head whose `Host` is this.
    Host(&'a str),
    /// A head with no `Host` (or not text where it should be).
    NoHost,
}

fn head(buf: &[u8]) -> Head<'_> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { return Head::More };
    // the request line first, then a header a line
    for line in buf[..end].split(|b| *b == b'\n').skip(1) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|b| *b == b':') else { continue };
        if line[..colon].eq_ignore_ascii_case(b"host") {
            return std::str::from_utf8(&line[colon + 1..]).map_or(Head::NoHost, |v| Head::Host(v.trim()));
        }
    }
    Head::NoHost
}

/// An answer to the guest's client, and the connection closed.
async fn answer(guest: &mut TcpStream, status: &str, text: &str) {
    let reply = format!("HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}\n", text.len() + 1);
    let _ = guest.write_all(reply.as_bytes()).await;
    let _ = guest.shutdown().await;
}

/// The addresses `name` is dialed at, by the guest's resolver (on a node,
/// the address its intercept knows the name by): never this forwarder's
/// own listeners.
pub async fn resolve(name: String, own_port: u16) -> std::io::Result<Vec<SocketAddr>> {
    let found = tokio::net::lookup_host((name.as_str(), DIAL_PORT)).await?;
    Ok(found.filter(|a| !a.ip().is_unspecified() && !(a.ip().is_loopback() && a.port() == own_port)).collect())
}

/// Serves `apps` on the guest's loopback until `stop` is true. A port the
/// guest already holds is said once, and the bridge goes on without it.
pub async fn serve(apps: Apps, stop: watch::Receiver<bool>) {
    let v4 = match TcpListener::bind((Ipv4Addr::LOCALHOST, apps.port)).await {
        Ok(l) => l,
        Err(e) => {
            crate::ev!("loopback.unavailable", { "zone": apps.zone, "port": apps.port, "error": e.to_string() });
            return;
        }
    };
    // a guest with no IPv6 loopback has only the one
    let v6 = TcpListener::bind((Ipv6Addr::LOCALHOST, apps.port)).await.ok();
    crate::ev!("loopback.serving", { "zone": apps.zone, "port": apps.port, "v6": v6.is_some() });
    let own = apps.port;
    let dial = move |name: String| resolve(name, own);
    let slots = Arc::new(Semaphore::new(limits::LOOPBACK_CONNECTIONS_MAX));
    let apps = Arc::new(apps);
    let second = v6.map(|l| tokio::spawn(serve_on(l, apps.clone(), dial, slots.clone(), stop.clone())));
    serve_on(v4, apps, dial, slots, stop).await;
    if let Some(h) = second {
        h.abort();
    }
}

/// Serves one listener: each connection forwarded (`forward`), at most
/// `LOOPBACK_CONNECTIONS_MAX` at once across `slots`' listeners.
pub async fn serve_on<R, Fut>(listener: TcpListener, apps: Arc<Apps>, dial: R, slots: Arc<Semaphore>, mut stop: watch::Receiver<bool>)
where
    R: Fn(String) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'static,
{
    // bounded by the bridge's life: one accept per pass, ended by `stop`
    loop {
        let slot = tokio::select! {
            s = slots.clone().acquire_owned() => s.expect("the semaphore is never closed"),
            _ = crate::net::stopped(&mut stop) => return,
        };
        let accepted = tokio::select! {
            a = listener.accept() => a,
            _ = crate::net::stopped(&mut stop) => return,
        };
        let Ok((guest, _)) = accepted else { continue };
        let (apps, dial) = (apps.clone(), dial.clone());
        tokio::spawn(async move {
            let _slot = slot;
            if let Err(why) = forward(guest, &apps, dial).await {
                crate::ev!("loopback.refused", { "why": why });
            }
        });
    }
}

/// One connection: its first request's host read, then dialed, and the
/// bytes passed both ways until either side ends. A refusal is answered to
/// the guest's client, and named in the error.
async fn forward<R, Fut>(mut guest: TcpStream, apps: &Apps, dial: R) -> Result<(), String>
where
    R: Fn(String) -> Fut,
    Fut: Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let wait = Duration::from_millis(limits::LOOPBACK_HEAD_WAIT_MS);
    // bounded by LOOPBACK_HEAD_MAX_BYTES and LOOPBACK_HEAD_WAIT_MS
    let name = loop {
        let host = match head(&buf) {
            Head::Host(h) => Some(h.to_string()),
            Head::NoHost => None,
            Head::More if buf.len() < limits::LOOPBACK_HEAD_MAX_BYTES => {
                let n = match tokio::time::timeout(wait, guest.read(&mut chunk)).await {
                    Ok(Ok(n)) => n,
                    // a client that sends nothing, or goes, is no one's to tell
                    Ok(Err(_)) | Err(_) => return Ok(()),
                };
                if n == 0 {
                    return Ok(());
                }
                buf.extend_from_slice(&chunk[..n]);
                continue;
            }
            Head::More => None,
        };
        let said = host.as_deref().unwrap_or("no host");
        match host.as_deref().and_then(|h| apps.name_of(h)) {
            Some(name) => break name,
            None => {
                let why = format!("this computer's port {} on its loopback is the platform's: it takes {} and the names under it, not {said}", apps.port, apps.zone);
                answer(&mut guest, "421 Misdirected Request", &why).await;
                return Err(why);
            }
        }
    };
    let (found, looked) = match dial(name.clone()).await {
        Ok(found) => {
            let n = found.len();
            (found, format!("{n} addresses found"))
        }
        Err(e) => (vec![], e.to_string()),
    };
    let mut up = None;
    for addr in &found {
        if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_millis(limits::LOOPBACK_DIAL_MS), TcpStream::connect(addr)).await {
            up = Some(s);
            break;
        }
    }
    let Some(mut up) = up else {
        let why = format!("{name} on port {DIAL_PORT} did not answer here ({looked})");
        answer(&mut guest, "502 Bad Gateway", &why).await;
        return Err(why);
    };
    let _ = (guest.set_nodelay(true), up.set_nodelay(true));
    up.write_all(&buf).await.map_err(|e| format!("{name}: {e}"))?;
    // ends when either side does: a page's live socket for the page's life
    let _ = tokio::io::copy_bidirectional(&mut guest, &mut up).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_come_from_a_localhost_origin_alone() {
        let local = |zone: &str, port| Some(Apps { zone: zone.into(), port });
        assert_eq!(Apps::of_origin("http://0123456789abcdef01234567--computer.fragment.localhost:8790"), local("fragment.localhost", 8790));
        // a branch's mark is in the label, never the zone
        assert_eq!(Apps::of_origin("http://abc--computer--next.Fragment.localhost:41234"), local("fragment.localhost", 41234));
        assert_eq!(Apps::of_origin("http://abc--computer.localhost"), local("localhost", 80));
        for elsewhere in [
            "https://abc--computer.fragment.localhost:8790",
            "https://abc--computer--b.finite.place",
            "http://abc--computer.home.arpa:8080",
            "http://abc--computer.localhost.example.com",
            "http://abc--computer.fragment.localhost:0",
            "http://localhost:8790",
            "http://127.0.0.1:8790",
            "",
        ] {
            assert_eq!(Apps::of_origin(elsewhere), None, "{elsewhere}");
        }
    }

    #[test]
    fn a_host_is_the_zones_on_its_port_or_refused() {
        let apps = Apps { zone: "fragment.localhost".into(), port: 8790 };
        assert_eq!(apps.name_of("todo--paul.fragment.localhost:8790").as_deref(), Some("todo--paul.fragment.localhost"));
        assert_eq!(apps.name_of(" Todo--Paul.Fragment.localhost.:8790").as_deref(), Some("todo--paul.fragment.localhost"));
        assert_eq!(apps.name_of("fragment.localhost:8790").as_deref(), Some("fragment.localhost"));
        for other in [
            "todo--paul.fragment.localhost",
            "todo--paul.fragment.localhost:8791",
            "todo--paul.fragment.localhost:port",
            "localhost:8790",
            "127.0.0.1:8790",
            "[::1]:8790",
            "xfragment.localhost:8790",
            ".fragment.localhost:8790",
            "a..fragment.localhost:8790",
            "todo--paul.fragment.localhost.example.com:8790",
            "a_b.fragment.localhost:8790",
            "",
        ] {
            assert_eq!(apps.name_of(other), None, "{other}");
        }
        let at_80 = Apps { zone: "fragment.localhost".into(), port: 80 };
        assert_eq!(at_80.name_of("a.fragment.localhost").as_deref(), Some("a.fragment.localhost"));
        assert_eq!(at_80.name_of("a.fragment.localhost:80").as_deref(), Some("a.fragment.localhost"));
    }

    #[test]
    fn a_head_names_its_host() {
        assert_eq!(head(b"GET / HTTP/1.1\r\nHost: a.fragment.localhost:8790\r\n"), Head::More);
        assert_eq!(head(b"GET / HTTP/1.1\r\nuser-agent: x\r\nHOST:  a.fragment.localhost:8790 \r\n\r\nbody"), Head::Host("a.fragment.localhost:8790"));
        // another header's bytes need not be text
        assert_eq!(head(b"GET / HTTP/1.1\r\ncookie: \xff\xfe\r\nhost: a\r\n\r\n"), Head::Host("a"));
        assert_eq!(head(b"GET / HTTP/1.1\r\naccept: */*\r\n\r\n"), Head::NoHost);
        assert_eq!(head(b"GET / HTTP/1.1\r\nhost: \xff\r\n\r\n"), Head::NoHost);
        // the request line is never a header, whatever it holds
        assert_eq!(head(b"GET /host:x HTTP/1.1\r\n\r\n"), Head::NoHost);
    }

    /// A server on loopback that answers each connection's first request
    /// with the `Host` it was sent, then echoes whatever comes after.
    async fn upstream() -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while matches!(head(&buf), Head::More) {
                        let n = s.read(&mut chunk).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let Head::Host(h) = head(&buf) else { panic!("a forwarded request keeps its host") };
                    let body = format!("host {h}");
                    s.write_all(format!("HTTP/1.1 101 Switching Protocols\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    // then a socket's frames, both ways
                    loop {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 || s.write_all(&chunk[..n]).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (addr, seen)
    }

    async fn read_some(s: &mut TcpStream) -> String {
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut chunk)).await.unwrap().unwrap();
        String::from_utf8_lossy(&chunk[..n]).into_owned()
    }

    // Goal: a connection to the platform's port on loopback reaches the
    // name its request names, its bytes as sent (its own `Host`, port and
    // all) and a socket's after; another host is refused there, reaching
    // nothing; a name nothing answers for is a 502. Method: the forwarder
    // on a loopback port of its own, its dial a stand-in for the guest's
    // resolver, a server that says what it was sent.
    #[tokio::test]
    async fn forwards_the_zones_names_and_refuses_the_rest() {
        crate::log::set_quiet(true);
        let (up, seen) = upstream().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let apps = Arc::new(Apps { zone: "fragment.localhost".into(), port });
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let noted = asked.clone();
        let dial = move |name: String| {
            noted.lock().unwrap().push(name.clone());
            async move { Ok(if name.starts_with("gone.") { vec![] } else { vec![up] }) }
        };
        let (stop_tx, stop) = watch::channel(false);
        tokio::spawn(serve_on(listener, apps, dial, Arc::new(Semaphore::new(4)), stop));

        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        // the head in two writes: it is read until whole
        s.write_all(b"GET /?view=t HTTP/1.1\r\nHost: todo--paul.fragment.localhost:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        s.write_all(format!("{port}\r\nupgrade: websocket\r\n\r\n").as_bytes()).await.unwrap();
        let first = read_some(&mut s).await;
        assert!(first.starts_with("HTTP/1.1 101") && first.ends_with(&format!("host todo--paul.fragment.localhost:{port}")), "{first}");
        s.write_all(b"a frame").await.unwrap();
        assert_eq!(read_some(&mut s).await, "a frame");
        assert_eq!(*asked.lock().unwrap(), vec!["todo--paul.fragment.localhost".to_string()]);

        for host in ["localhost", "127.0.0.1", "todo--paul.example.com"] {
            let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(format!("GET / HTTP/1.1\r\nhost: {host}:{port}\r\n\r\n").as_bytes()).await.unwrap();
            let refused = read_some(&mut s).await;
            assert!(refused.starts_with("HTTP/1.1 421") && refused.contains(&format!("not {host}:{port}")), "{refused}");
        }
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET / HTTP/1.1\r\nhost: gone.fragment.localhost:{port}\r\n\r\n").as_bytes()).await.unwrap();
        let gone = read_some(&mut s).await;
        assert!(gone.starts_with("HTTP/1.1 502") && gone.contains("gone.fragment.localhost on port 80"), "{gone}");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1, "a refused connection reaches nothing");
        assert_eq!(asked.lock().unwrap().len(), 2, "a refused host is never even looked up");

        // a head as long as the bound and not done is refused, not read on
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut long = b"GET / HTTP/1.1\r\nx: ".to_vec();
        long.resize(limits::LOOPBACK_HEAD_MAX_BYTES, b'a');
        s.write_all(&long).await.unwrap();
        let cut = read_some(&mut s).await;
        assert!(cut.starts_with("HTTP/1.1 421") && cut.contains("not no host"), "{cut}");
        let _ = stop_tx.send(true);
    }

    // Goal: the forwarder never dials itself: a resolver that answers a
    // `.localhost` name with loopback (as RFC 6761 has a host's own do) on
    // the forwarder's own port is no address to dial. Method: the filter on
    // addresses as `lookup_host` gives them.
    #[tokio::test]
    async fn never_its_own_listener() {
        // an address as written is its own lookup
        assert_eq!(resolve("127.0.0.1".into(), DIAL_PORT).await.unwrap(), vec![]);
        assert_eq!(resolve("127.0.0.1".into(), 8790).await.unwrap(), vec![SocketAddr::from((Ipv4Addr::LOCALHOST, DIAL_PORT))]);
        assert_eq!(resolve("0.0.0.0".into(), 8790).await.unwrap(), vec![]);
        assert_eq!(resolve("198.18.0.7".into(), DIAL_PORT).await.unwrap(), vec![SocketAddr::from(([198, 18, 0, 7], DIAL_PORT))]);
    }
}
