//! DNS for the zone: the box's server is authoritative for its zone alone.
//!
//! The apex and every name one label under it answer the box's address
//! (A for an IPv4 address, AAAA for an IPv6 one; the other type is
//! NODATA), the apex its SOA and NS too; a deeper name does not exist
//! (NXDOMAIN). Every other name is refused (REFUSED): this is no resolver
//! and forwards nothing, so it is never an open one. A device asks it for
//! the zone alone (a tailnet's split DNS, a resolver's stub zone, or
//! macOS's `/etc/resolver/<zone>`) and asks its own resolver for the rest.
//!
//! Wire format: simple-dns. UDP and TCP (RFC 7766's two-byte lengths,
//! several queries a connection), each bounded.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use simple_dns::rdata::{RData, A, AAAA, NS, SOA};
use simple_dns::{Name, Packet, PacketFlag, Question, ResourceRecord, CLASS, OPCODE, QCLASS, QTYPE, RCODE, TYPE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Semaphore;

use crate::zone::{Place, Zone};

/// The zone's records' TTL, and its negative answers' (SOA minimum).
pub const TTL: u32 = 60;
/// A datagram read whole: more than any query needs.
pub const UDP_BYTES_MAX: usize = 4096;
/// An answer at most: within a plain UDP datagram, so none is truncated.
pub const ANSWER_BYTES_MAX: usize = 512;
/// TCP clients at once, the queries one connection may send, and how long
/// it may idle between them.
pub const TCP_CONNECTIONS_MAX: usize = 64;
pub const TCP_QUERIES_MAX: usize = 100;
pub const TCP_IDLE: Duration = Duration::from_secs(10);
/// The EDNS payload this server says it takes (DNS Flag Day 2020's).
const EDNS_PAYLOAD: u16 = 1232;

/// What answers the zone's names.
pub struct Answerer {
    zone: Zone,
    address: IpAddr,
    /// The SOA's serial: the start's second. Nothing transfers the zone.
    serial: u32,
}

/// What to do with one message.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    /// Answer with these bytes.
    Answer(Vec<u8>),
    /// Not a query (a response, or too short for an id): no answer.
    Drop,
}

impl Answerer {
    pub fn new(zone: Zone, address: IpAddr) -> Answerer {
        let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| (d.as_secs() & 0xffff_ffff) as u32);
        Answerer { zone, address, serial }
    }

    /// The answer to `query`, a DNS message as it came off the wire.
    pub fn reply(&self, query: &[u8]) -> Reply {
        let Ok(msg) = Packet::parse(query) else {
            // an id and a query's header to answer to, or nothing
            if query.len() >= 12 && query[2] & 0x80 == 0 {
                let mut out = Packet::new_reply(u16::from_be_bytes([query[0], query[1]]));
                *out.rcode_mut() = RCODE::FormatError;
                return Reply::Answer(encode(&out));
            }
            return Reply::Drop;
        };
        if msg.has_flags(PacketFlag::RESPONSE) {
            return Reply::Drop;
        }
        let mut out = Packet::new_reply(msg.id());
        if msg.has_flags(PacketFlag::RECURSION_DESIRED) {
            out.set_flags(PacketFlag::RECURSION_DESIRED);
        }
        out.questions = msg.questions.clone();
        if msg.opt().is_some() {
            *out.opt_mut() = Some(simple_dns::rdata::OPT { opt_codes: vec![], udp_packet_size: EDNS_PAYLOAD, version: 0 });
        }
        let refuse = |mut out: Packet, code: RCODE| {
            *out.rcode_mut() = code;
            Reply::Answer(encode(&out))
        };
        if msg.opcode() != OPCODE::StandardQuery {
            *out.opcode_mut() = msg.opcode();
            return refuse(out, RCODE::NotImplemented);
        }
        let [q] = msg.questions.as_slice() else { return refuse(out, RCODE::FormatError) };
        if !matches!(q.qclass, QCLASS::CLASS(CLASS::IN) | QCLASS::ANY) {
            return refuse(out, RCODE::Refused);
        }
        let place = if q.qname.is_valid_hostname() { self.zone.place(&q.qname.to_string()) } else { Place::Outside };
        let answer = match place {
            Place::Outside => return refuse(out, RCODE::Refused),
            Place::Deeper => {
                out.set_flags(PacketFlag::AUTHORITATIVE_ANSWER);
                out.name_servers.push(self.soa_record());
                return refuse(out, RCODE::NameError);
            }
            Place::Apex | Place::Under(_) => self.answer(&place, q),
        };
        out.set_flags(PacketFlag::AUTHORITATIVE_ANSWER);
        match answer {
            Some(record) => out.answers.push(record),
            // every name here exists, with an address and nothing else
            None => out.name_servers.push(self.soa_record()),
        }
        let bytes = encode(&out);
        assert!(bytes.len() <= ANSWER_BYTES_MAX, "the zone's answers fit a plain UDP datagram");
        Reply::Answer(bytes)
    }

    /// The one record a question in the zone has, or none (NODATA).
    fn answer(&self, place: &Place, q: &Question<'_>) -> Option<ResourceRecord<'static>> {
        let name = q.qname.clone().into_owned();
        let record = |rdata: RData<'static>| Some(ResourceRecord::new(name.clone(), CLASS::IN, TTL, rdata));
        let address = || match self.address {
            IpAddr::V4(a) => RData::A(A { address: a.into() }),
            IpAddr::V6(a) => RData::AAAA(AAAA { address: a.into() }),
        };
        let apex = *place == Place::Apex;
        match (q.qtype, self.address) {
            (QTYPE::TYPE(TYPE::A), IpAddr::V4(_)) | (QTYPE::TYPE(TYPE::AAAA), IpAddr::V6(_)) | (QTYPE::ANY, _) => record(address()),
            (QTYPE::TYPE(TYPE::SOA), _) if apex => record(RData::SOA(self.soa())),
            (QTYPE::TYPE(TYPE::NS), _) if apex => record(RData::NS(NS(self.apex_name()))),
            _ => None,
        }
    }

    fn apex_name(&self) -> Name<'static> {
        Name::new(self.zone.name()).expect("a zone is a name").into_owned()
    }

    fn soa(&self) -> SOA<'static> {
        let under = |label: &str| Name::new(&format!("{label}.{}", self.zone)).expect("a label under the zone is a name").into_owned();
        // the zone's own name server is the apex itself
        SOA { mname: self.apex_name(), rname: under("hostmaster"), serial: self.serial, refresh: 3600, retry: 600, expire: 86_400, minimum: TTL }
    }

    fn soa_record(&self) -> ResourceRecord<'static> {
        ResourceRecord::new(self.apex_name(), CLASS::IN, TTL, RData::SOA(self.soa()))
    }
}

fn encode(m: &Packet<'_>) -> Vec<u8> {
    m.build_bytes_vec_compressed().expect("a message made here encodes")
}

/// Serves UDP on `socket` until the task is dropped: each datagram
/// answered as it comes (answering is a lookup, never a wait).
pub async fn serve_udp(socket: UdpSocket, answerer: Arc<Answerer>) {
    let mut buf = vec![0u8; UDP_BYTES_MAX];
    // unbounded by design: a server's receive loop, each pass one datagram
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
        if let Reply::Answer(bytes) = answerer.reply(&buf[..n]) {
            let _ = socket.send_to(&bytes, from).await;
        }
    }
}

/// Serves TCP on `listener` until the task is dropped.
pub async fn serve_tcp(listener: TcpListener, answerer: Arc<Answerer>) {
    let permits = Arc::new(Semaphore::new(TCP_CONNECTIONS_MAX));
    // unbounded by design: a server's accept loop, each connection bounded
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
        let answerer = answerer.clone();
        tokio::spawn(async move {
            let _ = tcp_connection(stream, &answerer).await;
            drop(permit);
        });
    }
}

async fn tcp_connection(mut stream: TcpStream, answerer: &Answerer) -> std::io::Result<()> {
    for _ in 0..TCP_QUERIES_MAX {
        let Ok(query) = tokio::time::timeout(TCP_IDLE, read_framed(&mut stream)).await else { return Ok(()) };
        let Some(query) = query? else { return Ok(()) };
        match answerer.reply(&query) {
            Reply::Answer(bytes) => write_framed(&mut stream, &bytes).await?,
            Reply::Drop => return Ok(()),
        }
    }
    Ok(())
}

/// One TCP message, or `None` at a clean end of stream.
async fn read_framed(stream: &mut TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 2];
    match stream.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
    stream.read_exact(&mut msg).await?;
    Ok(Some(msg))
}

async fn write_framed(stream: &mut TcpStream, msg: &[u8]) -> std::io::Result<()> {
    let len = u16::try_from(msg.len()).expect("an answer made here fits a TCP frame");
    let mut framed = Vec::with_capacity(msg.len() + 2);
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(msg);
    stream.write_all(&framed).await
}

/// What a lookup heard.
#[derive(Debug, PartialEq, Eq)]
pub enum Heard {
    /// The name's addresses (none: the name exists without one of that type).
    Addresses(Vec<IpAddr>),
    /// The name does not exist (NXDOMAIN).
    NoSuchName,
    /// The server refused it (REFUSED).
    Refused,
    /// Any other answer code.
    Other(u8),
}

#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    #[error("{0:?} is no DNS name")]
    Name(String),
    #[error("asking {server}: {source}")]
    Io { server: SocketAddr, source: std::io::Error },
    #[error("{server} answered something that is not this query's answer")]
    Malformed { server: SocketAddr },
}

/// The record type a lookup asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    A,
    Aaaa,
}

/// Asks `server` for `name`'s `ask` over UDP, as a device's stub
/// resolver does (one query, no retry, waiting `wait`): the e2e's clients
/// find the box's names so, and its operator can check them.
pub fn lookup(server: SocketAddr, name: &str, ask: Ask, wait: Duration) -> Result<Heard, LookupError> {
    let qname = Name::new(name).map_err(|_| LookupError::Name(name.to_string()))?;
    let id = u16::from_be_bytes(random_id());
    let mut q = Packet::new_query(id);
    q.set_flags(PacketFlag::RECURSION_DESIRED);
    let qtype = match ask {
        Ask::A => TYPE::A,
        Ask::Aaaa => TYPE::AAAA,
    };
    q.questions.push(Question::new(qname, QTYPE::TYPE(qtype), QCLASS::CLASS(CLASS::IN), false));
    let io = |source| LookupError::Io { server, source };
    let local: SocketAddr = if server.is_ipv4() { ([127, 0, 0, 1], 0).into() } else { (std::net::Ipv6Addr::LOCALHOST, 0).into() };
    let socket = std::net::UdpSocket::bind(local).map_err(io)?;
    socket.set_read_timeout(Some(wait)).map_err(io)?;
    socket.connect(server).map_err(io)?;
    socket.send(&encode(&q)).map_err(io)?;
    let mut buf = vec![0u8; UDP_BYTES_MAX];
    let n = socket.recv(&mut buf).map_err(io)?;
    let answer = Packet::parse(&buf[..n]).map_err(|_| LookupError::Malformed { server })?;
    if answer.id() != id || !answer.has_flags(PacketFlag::RESPONSE) {
        return Err(LookupError::Malformed { server });
    }
    Ok(match answer.rcode() {
        RCODE::NoError => Heard::Addresses(
            answer
                .answers
                .iter()
                .filter_map(|r| match &r.rdata {
                    RData::A(a) => Some(IpAddr::V4(a.address.into())),
                    RData::AAAA(a) => Some(IpAddr::V6(a.address.into())),
                    _ => None,
                })
                .collect(),
        ),
        RCODE::NameError => Heard::NoSuchName,
        RCODE::Refused => Heard::Refused,
        other => Heard::Other(other as u8),
    })
}

fn random_id() -> [u8; 2] {
    let mut b = [0u8; 2];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).expect("the system's randomness");
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const BOX: Ipv4Addr = Ipv4Addr::new(100, 64, 0, 7);

    fn answerer() -> Answerer {
        Answerer::new(Zone::parse("fragment.home.arpa").unwrap(), IpAddr::V4(BOX))
    }

    fn query(name: &str, qtype: QTYPE, edns: bool) -> Vec<u8> {
        let mut m = Packet::new_query(0x1234);
        m.set_flags(PacketFlag::RECURSION_DESIRED);
        m.questions.push(Question::new(Name::new(name).unwrap(), qtype, QCLASS::CLASS(CLASS::IN), false));
        if edns {
            *m.opt_mut() = Some(simple_dns::rdata::OPT { opt_codes: vec![], udp_packet_size: 4096, version: 0 });
        }
        m.build_bytes_vec().unwrap()
    }

    /// The answer's bytes (a test parses them where it reads them).
    fn answered(a: &Answerer, q: &[u8]) -> Vec<u8> {
        match a.reply(q) {
            Reply::Answer(b) => b,
            Reply::Drop => panic!("dropped"),
        }
    }

    /// `answered`, parsed in place.
    macro_rules! ask {
        ($m:ident = $a:expr, $q:expr) => {
            let bytes = answered($a, $q);
            let $m = Packet::parse(&bytes).unwrap();
        };
    }

    fn rcode(a: &Answerer, q: &[u8]) -> RCODE {
        ask!(m = a, q);
        m.rcode()
    }

    fn addresses(m: &Packet<'_>) -> Vec<IpAddr> {
        m.answers
            .iter()
            .filter_map(|r| match &r.rdata {
                RData::A(a) => Some(IpAddr::V4(a.address.into())),
                RData::AAAA(a) => Some(IpAddr::V6(a.address.into())),
                _ => None,
            })
            .collect()
    }

    /// Goal (valid): the apex and a name one label under it, in any case,
    /// answer the box's address, authoritatively, with the query's id and
    /// question. Replay: the same query again answers the same.
    #[test]
    fn the_zone_answers_the_box() {
        let a = answerer();
        for name in ["fragment.home.arpa", "todo--k8ew.fragment.home.arpa", "TODO--K8EW.Fragment.Home.Arpa"] {
            let q = query(name, QTYPE::TYPE(TYPE::A), false);
            ask!(m = &a, &q);
            assert_eq!(m.id(), 0x1234);
            assert_eq!(m.rcode(), RCODE::NoError, "{name}");
            assert!(m.has_flags(PacketFlag::AUTHORITATIVE_ANSWER) && m.has_flags(PacketFlag::RECURSION_DESIRED) && !m.has_flags(PacketFlag::RECURSION_AVAILABLE));
            assert_eq!(addresses(&m), [IpAddr::V4(BOX)], "{name}");
            assert_eq!(m.answers[0].ttl, TTL);
            assert_eq!(m.questions.len(), 1);
            assert_eq!(a.reply(&q), a.reply(&q), "the same answer again");
        }
    }

    /// Goal: another type is NODATA with the zone's SOA (an IPv6-first
    /// client falls back at once), the apex has its SOA and NS, an EDNS
    /// query gets EDNS back; an IPv6 box answers AAAA.
    #[test]
    fn other_types_are_nodata_and_the_apex_has_soa_and_ns() {
        let a = answerer();
        ask!(m = &a, &query("x--y.fragment.home.arpa", QTYPE::TYPE(TYPE::AAAA), true));
        assert_eq!(m.rcode(), RCODE::NoError);
        assert!(m.answers.is_empty());
        assert!(matches!(m.name_servers[0].rdata, RData::SOA(_)));
        assert!(m.opt().is_some(), "EDNS answered with EDNS");
        ask!(soa = &a, &query("fragment.home.arpa", QTYPE::TYPE(TYPE::SOA), false));
        assert!(matches!(soa.answers[0].rdata, RData::SOA(_)));
        ask!(ns = &a, &query("fragment.home.arpa", QTYPE::TYPE(TYPE::NS), false));
        assert!(matches!(&ns.answers[0].rdata, RData::NS(n) if n.0.to_string() == "fragment.home.arpa"));
        let v6 = Answerer::new(Zone::parse("fragment.home.arpa").unwrap(), "fd7a:115c:a1e0::7".parse().unwrap());
        ask!(six = &v6, &query("fragment.home.arpa", QTYPE::TYPE(TYPE::AAAA), false));
        assert_eq!(addresses(&six), ["fd7a:115c:a1e0::7".parse::<IpAddr>().unwrap()]);
        ask!(four = &v6, &query("fragment.home.arpa", QTYPE::TYPE(TYPE::A), false));
        assert!(four.answers.is_empty());
    }

    /// Goal (invalid): every name outside the zone is refused (no
    /// forwarding: no open resolver), a look-alike included; a deeper name
    /// does not exist; a response is never answered; garbage with an id is
    /// FORMERR; two questions, another class or opcode are refused.
    #[test]
    fn other_names_are_refused_and_malformed_queries_too() {
        let a = answerer();
        for name in ["example.com", "home.arpa", "evilfragment.home.arpa", "fragment.home.arpa.evil.example"] {
            ask!(m = &a, &query(name, QTYPE::TYPE(TYPE::A), false));
            assert_eq!(m.rcode(), RCODE::Refused, "{name}");
            assert!(m.answers.is_empty() && !m.has_flags(PacketFlag::AUTHORITATIVE_ANSWER));
        }
        ask!(deep = &a, &query("a.b.fragment.home.arpa", QTYPE::TYPE(TYPE::A), false));
        assert_eq!(deep.rcode(), RCODE::NameError);
        assert!(matches!(deep.name_servers[0].rdata, RData::SOA(_)));
        let mut resp = query("fragment.home.arpa", QTYPE::TYPE(TYPE::A), false);
        resp[2] |= 0x80;
        assert_eq!(a.reply(&resp), Reply::Drop, "a response is not answered");
        assert_eq!(a.reply(&[1, 2, 3]), Reply::Drop);
        let mut junk = vec![0xab, 0xcd, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        junk.extend_from_slice(&[0xff; 7]);
        ask!(m = &a, &junk);
        assert_eq!((m.id(), m.rcode()), (0xabcd, RCODE::FormatError));
        let mut two = Packet::new_query(7);
        for n in ["fragment.home.arpa", "x--y.fragment.home.arpa"] {
            two.questions.push(Question::new(Name::new(n).unwrap(), QTYPE::TYPE(TYPE::A), QCLASS::CLASS(CLASS::IN), false));
        }
        assert_eq!(rcode(&a, &two.build_bytes_vec().unwrap()), RCODE::FormatError);
        let mut chaos = Packet::new_query(8);
        chaos.questions.push(Question::new(Name::new("fragment.home.arpa").unwrap(), QTYPE::TYPE(TYPE::A), QCLASS::CLASS(CLASS::CH), false));
        assert_eq!(rcode(&a, &chaos.build_bytes_vec().unwrap()), RCODE::Refused);
        let mut notify = Packet::new_query(9);
        *notify.opcode_mut() = OPCODE::Notify;
        notify.questions.push(Question::new(Name::new("fragment.home.arpa").unwrap(), QTYPE::TYPE(TYPE::SOA), QCLASS::CLASS(CLASS::IN), false));
        assert_eq!(rcode(&a, &notify.build_bytes_vec().unwrap()), RCODE::NotImplemented);
    }
}
