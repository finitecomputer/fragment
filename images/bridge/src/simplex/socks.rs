//! The SimpleX cores' way out: a SOCKS5 proxy on loopback, which every core
//! connects through (`--socks-proxy`).
//!
//! Cloudflare Containers reset a TLS hello on 443 that names no server, and
//! a SimpleX server on 443 takes its protocol only from such a hello
//! (docs/bridge.md, "SimpleX"), so a core on a computer reaches no XFTP
//! server straight out, and no file arrives. Each connection a core asks
//! for goes on one of two ways:
//! - to a host and port the platform's XFTP relay carries (its view's
//!   `relay`: SimpleX's preset XFTP servers, on 443), through the relay
//!   (`GET /api/computer/relay`), its bytes untouched both ways: the core's
//!   TLS runs end to end, to the server's own certificate;
//! - to anything else (SMP servers, on 5223), straight out of the computer,
//!   as the core would go itself.
//!
//! It speaks SOCKS5's CONNECT alone (RFC 1928), with no authentication or
//! with a user name and password it reads and ignores (RFC 1929: the core
//! sends some, to keep its sessions apart). What it refuses, it answers
//! with SOCKS's own reply. At most `SESSIONS_MAX` at once; a session whose
//! greeting and request take longer than `HANDSHAKE_MS` is dropped. It logs
//! each relay's host, bytes and time, never what is carried.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio_tungstenite::tungstenite::Message;

use crate::api::{Api, RelayOffer};
use crate::net::{self, ClientWs};

/// Sessions at once, at most: the cores' connections to their servers.
pub const SESSIONS_MAX: usize = 64;
/// A session's greeting and request are read within this, or it is dropped.
const HANDSHAKE_MS: u64 = 10_000;
/// A connection straight out is made within this, or refused.
const CONNECT_MS: u64 = 15_000;
/// What is read from a core at a time, and sent on as one frame.
const CHUNK_BYTES: usize = 16 * 1024;
/// A name in a request is at most this long (RFC 1928: one length byte).
const NAME_MAX_BYTES: usize = 255;

/// SOCKS5's replies (RFC 1928, section 6).
pub mod reply {
    pub const SUCCEEDED: u8 = 0;
    pub const FAILURE: u8 = 1;
    pub const NOT_ALLOWED: u8 = 2;
    pub const HOST_UNREACHABLE: u8 = 4;
    pub const COMMAND: u8 = 7;
    pub const ADDRESS: u8 = 8;
}

/// Where a session asks to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    Name(String),
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

/// A session's request: where to, and on which port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub dest: Dest,
    pub port: u16,
}

impl Ask {
    /// Through the relay, when it carries this host and port (the host as
    /// its list names it); else straight out.
    pub fn relayed<'a>(&self, offer: Option<&'a RelayOffer>) -> Option<&'a str> {
        match (&self.dest, offer) {
            (Dest::Name(host), Some(o)) => o.carried(host, self.port),
            _ => None,
        }
    }
}

/// Reads a session's greeting and request from `io`, answering the
/// greeting: where it asks to go. A refusal is answered on `io` (its reply)
/// and is the error.
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(io: &mut S) -> Result<Ask, String> {
    let mut head = [0u8; 2];
    io.read_exact(&mut head).await.map_err(|e| format!("its greeting: {e}"))?;
    if head[0] != 5 {
        return Err(format!("SOCKS version {}, not 5", head[0]));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    io.read_exact(&mut methods).await.map_err(|e| format!("its methods: {e}"))?;
    // a user name and password, when it offers them (the core's), else none
    let method = if methods.contains(&2) {
        2
    } else if methods.contains(&0) {
        0
    } else {
        let _ = io.write_all(&[5, 0xff]).await;
        return Err("no method this takes".into());
    };
    io.write_all(&[5, method]).await.map_err(|e| e.to_string())?;
    if method == 2 {
        let mut ver = [0u8; 2];
        io.read_exact(&mut ver).await.map_err(|e| format!("its credentials: {e}"))?;
        let mut user = vec![0u8; usize::from(ver[1])];
        io.read_exact(&mut user).await.map_err(|e| format!("its user name: {e}"))?;
        let mut len = [0u8; 1];
        io.read_exact(&mut len).await.map_err(|e| format!("its password: {e}"))?;
        let mut password = vec![0u8; usize::from(len[0])];
        io.read_exact(&mut password).await.map_err(|e| format!("its password: {e}"))?;
        if ver[0] != 1 {
            let _ = io.write_all(&[1, 1]).await;
            return Err(format!("credentials version {}, not 1", ver[0]));
        }
        io.write_all(&[1, 0]).await.map_err(|e| e.to_string())?;
    }
    let mut req = [0u8; 4];
    io.read_exact(&mut req).await.map_err(|e| format!("its request: {e}"))?;
    if req[0] != 5 {
        return Err(format!("SOCKS version {}, not 5", req[0]));
    }
    if req[1] != 1 {
        answer(io, reply::COMMAND).await;
        return Err(format!("command {}: only CONNECT", req[1]));
    }
    let dest = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            io.read_exact(&mut a).await.map_err(|e| format!("its address: {e}"))?;
            Dest::V4(Ipv4Addr::from(a))
        }
        3 => {
            let mut len = [0u8; 1];
            io.read_exact(&mut len).await.map_err(|e| format!("its name: {e}"))?;
            let mut name = vec![0u8; usize::from(len[0])];
            io.read_exact(&mut name).await.map_err(|e| format!("its name: {e}"))?;
            match String::from_utf8(name) {
                Ok(n) if !n.is_empty() && n.len() <= NAME_MAX_BYTES && n.bytes().all(|b| b.is_ascii_graphic()) => Dest::Name(n),
                _ => {
                    answer(io, reply::ADDRESS).await;
                    return Err("a name that is no host's".into());
                }
            }
        }
        4 => {
            let mut a = [0u8; 16];
            io.read_exact(&mut a).await.map_err(|e| format!("its address: {e}"))?;
            Dest::V6(Ipv6Addr::from(a))
        }
        t => {
            answer(io, reply::ADDRESS).await;
            return Err(format!("address type {t}"));
        }
    };
    let mut port = [0u8; 2];
    io.read_exact(&mut port).await.map_err(|e| format!("its port: {e}"))?;
    Ok(Ask { dest, port: u16::from_be_bytes(port) })
}

/// A request's reply, its bound address none (0.0.0.0:0).
async fn answer<S: AsyncWrite + Unpin>(io: &mut S, code: u8) {
    let _ = io.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await;
}

/// Serves `listener` until `stop`: each session handshaken, then sent on
/// by the relay `offer` says (the computer's view's, as last read).
pub async fn serve(listener: TcpListener, api: Api, offer: watch::Receiver<Option<RelayOffer>>, mut stop: watch::Receiver<bool>) {
    let sessions = Arc::new(Semaphore::new(SESSIONS_MAX));
    // bounded by `stop`, and each session by `SESSIONS_MAX`
    loop {
        let accepted = tokio::select! {
            a = listener.accept() => a,
            _ = net::stopped(&mut stop) => return,
        };
        let Ok((tcp, _)) = accepted else { continue };
        let Ok(permit) = sessions.clone().try_acquire_owned() else {
            crate::ev!("simplex.socks_bounded", { "max": SESSIONS_MAX });
            continue;
        };
        let (api, offer, stop) = (api.clone(), offer.clone(), stop.clone());
        tokio::spawn(async move {
            session(tcp, &api, &offer, stop).await;
            drop(permit);
        });
    }
}

async fn session(mut tcp: TcpStream, api: &Api, offer: &watch::Receiver<Option<RelayOffer>>, stop: watch::Receiver<bool>) {
    let _ = tcp.set_nodelay(true);
    let ask = match tokio::time::timeout(Duration::from_millis(HANDSHAKE_MS), handshake(&mut tcp)).await {
        Ok(Ok(ask)) => ask,
        Ok(Err(e)) => return crate::ev!("simplex.socks_refused", { "error": e }),
        Err(_) => return crate::ev!("simplex.socks_refused", { "error": "no request in time" }),
    };
    let relayed = ask.relayed(offer.borrow().as_ref()).map(str::to_string);
    match relayed {
        Some(host) => {
            let ws = match api.relay(&host, ask.port).await {
                Ok(ws) => ws,
                Err(e) => {
                    answer(&mut tcp, reply::NOT_ALLOWED).await;
                    return crate::ev!("simplex.relay_refused", { "host": host, "error": e });
                }
            };
            answer(&mut tcp, reply::SUCCEEDED).await;
            let t0 = Instant::now();
            let (up, down) = pipe(tcp, ws, stop).await;
            crate::ev!("simplex.relay", { "host": host, "port": ask.port, "up": up, "down": down, "ms": t0.elapsed().as_millis() as u64 });
        }
        None => {
            let out = match &ask.dest {
                Dest::Name(n) => tokio::time::timeout(Duration::from_millis(CONNECT_MS), TcpStream::connect((n.as_str(), ask.port))).await,
                Dest::V4(a) => tokio::time::timeout(Duration::from_millis(CONNECT_MS), TcpStream::connect((*a, ask.port))).await,
                Dest::V6(a) => tokio::time::timeout(Duration::from_millis(CONNECT_MS), TcpStream::connect((*a, ask.port))).await,
            };
            let Ok(Ok(mut out)) = out else {
                return answer(&mut tcp, reply::HOST_UNREACHABLE).await;
            };
            let _ = out.set_nodelay(true);
            answer(&mut tcp, reply::SUCCEEDED).await;
            let mut stop = stop;
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut tcp, &mut out) => {}
                _ = net::stopped(&mut stop) => {}
            }
        }
    }
}

/// A core's connection and the relay's socket, piped both ways until either
/// ends (or `stop`): the bytes sent up and down.
async fn pipe(mut tcp: TcpStream, ws: ClientWs, mut stop: watch::Receiver<bool>) -> (u64, u64) {
    let (mut rd, mut wr) = tcp.split();
    let (mut sink, mut stream) = ws.split();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let (mut up, mut down) = (0u64, 0u64);
    // bounded by either side's end, the relay's own bounds, or `stop`
    loop {
        tokio::select! {
            n = rd.read(&mut buf) => match n {
                Ok(0) | Err(_) => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
                Ok(n) => {
                    up += n as u64;
                    if sink.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                        break;
                    }
                }
            },
            m = stream.next() => match m {
                Some(Ok(Message::Binary(b))) => {
                    down += b.len() as u64;
                    if wr.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = net::stopped(&mut stop) => break,
        }
    }
    let _ = wr.shutdown().await;
    (up, down)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(mut io: tokio::io::DuplexStream) -> Vec<u8> {
        let mut out = vec![];
        let _ = io.read_to_end(&mut out).await;
        out
    }

    /// The core's own handshake: a user name and password it offers, then
    /// CONNECT to a name.
    fn cores(host: &str, port: u16) -> Vec<u8> {
        let mut b = vec![5, 2, 0, 2, 1, 3, b'u', b's', b'r', 2, b'p', b'w', 5, 1, 0, 3, host.len() as u8];
        b.extend(host.as_bytes());
        b.extend(port.to_be_bytes());
        b
    }

    async fn shake(bytes: Vec<u8>) -> (Result<Ask, String>, Vec<u8>) {
        let (mut ours, mut theirs) = tokio::io::duplex(4096);
        theirs.write_all(&bytes).await.expect("written");
        // all it will send: a request cut short ends, never waits
        theirs.shutdown().await.expect("its side closed");
        let r = handshake(&mut ours).await;
        drop(ours);
        (r, read_all(theirs).await)
    }

    /// Goal: the core's handshake (a user name and password, then CONNECT to
    /// a name) and a plain one (no authentication, an address) are read as
    /// what they ask, each answered as SOCKS5 answers; anything else (another
    /// version, no method it takes, another command, a bad name, an unknown
    /// address type, a request cut short) is refused with SOCKS's own reply,
    /// or dropped.
    #[tokio::test]
    async fn its_handshake_reads_the_cores_and_refuses_the_rest() {
        let (r, said) = shake(cores("xftp3.simplex.im", 443)).await;
        assert_eq!(r, Ok(Ask { dest: Dest::Name("xftp3.simplex.im".into()), port: 443 }));
        assert_eq!(said, [5, 2, 1, 0], "the user name and password method, then taken");
        let (r, said) = shake(vec![5, 1, 0, 5, 1, 0, 1, 23, 128, 248, 112, 0x14, 0x5f]).await;
        assert_eq!(r, Ok(Ask { dest: Dest::V4(Ipv4Addr::new(23, 128, 248, 112)), port: 5215 }));
        assert_eq!(said, [5, 0]);
        let mut v6 = vec![5, 1, 0, 5, 1, 0, 4];
        v6.extend(Ipv6Addr::LOCALHOST.octets());
        v6.extend(5223u16.to_be_bytes());
        assert_eq!(shake(v6).await.0, Ok(Ask { dest: Dest::V6(Ipv6Addr::LOCALHOST), port: 5223 }));

        let (r, said) = shake(vec![4, 1, 0]).await;
        assert!(r.is_err() && said.is_empty(), "SOCKS4: dropped");
        let (r, said) = shake(vec![5, 1, 1]).await;
        assert!(r.is_err() && said == [5, 0xff], "GSSAPI alone: no method it takes");
        let (r, said) = shake(vec![5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80]).await;
        assert!(r.is_err() && said == [5, 0, 5, reply::COMMAND, 0, 1, 0, 0, 0, 0, 0, 0], "BIND: refused");
        let (r, said) = shake(vec![5, 1, 0, 5, 1, 0, 9]).await;
        assert!(r.is_err() && said[2..4] == [5, reply::ADDRESS], "an unknown address type: refused");
        for bad in [&b"bad host"[..], b"", b"\xff\xfe"] {
            let mut b = vec![5, 1, 0, 5, 1, 0, 3, bad.len() as u8];
            b.extend(bad);
            b.extend(443u16.to_be_bytes());
            let (r, said) = shake(b).await;
            assert!(r.is_err() && said[2..4] == [5, reply::ADDRESS], "{bad:?}: no host's name");
        }
        let mut cut = cores("xftp3.simplex.im", 443);
        cut.truncate(cut.len() - 1);
        assert!(shake(cut).await.0.is_err(), "a request cut short");
    }

    /// Goal: a connection goes through the relay only to a host and port the
    /// relay carries (any case), and only from a platform that offers one;
    /// anything else (another port, another host, an address, no offer)
    /// goes straight out.
    #[test]
    fn only_what_the_relay_carries_goes_through_it() {
        let offer = RelayOffer { hosts: vec!["xftp3.simplex.im".into()], port: 443 };
        let ask = |dest: Dest, port| Ask { dest, port };
        assert_eq!(ask(Dest::Name("xftp3.simplex.im".into()), 443).relayed(Some(&offer)), Some("xftp3.simplex.im"));
        assert_eq!(ask(Dest::Name("XFTP3.simplex.im".into()), 443).relayed(Some(&offer)), Some("xftp3.simplex.im"), "named as its list names it");
        assert_eq!(ask(Dest::Name("xftp3.simplex.im".into()), 5223).relayed(Some(&offer)), None);
        assert_eq!(ask(Dest::Name("smp4.simplex.im".into()), 443).relayed(Some(&offer)), None);
        assert_eq!(ask(Dest::V4(Ipv4Addr::new(23, 128, 248, 115)), 443).relayed(Some(&offer)), None);
        assert_eq!(ask(Dest::Name("xftp3.simplex.im".into()), 443).relayed(None), None, "no relay offered: straight out");
    }
}
