//! The XFTP relay (docs/computers.md, "The XFTP relay"): raw TCP from a
//! computer's SimpleX core to SimpleX's file servers, through the platform.
//!
//! Cloudflare Containers reset a TLS hello on port 443 that names no server
//! (SNI), and a SimpleX server on 443 takes its protocol only from such a
//! hello, so a computer reaches no XFTP server directly (measured on a
//! preview computer, 2026-10-25; docs/explorations/simplex-complexity.md,
//! section 8). A Worker's `connect()` is raw TCP that nothing inspects: the
//! computer's bridge hands the platform the core's bytes over a WebSocket
//! (`GET /api/computer/relay?host=&port=`), and the platform pipes them to
//! the server untouched. The core's TLS runs end to end, checked against the
//! server's own certificate fingerprint, and XFTP encrypts each file's
//! chunks besides: the platform sees the host, the port, the byte counts and
//! the times, never what is sent.
//!
//! No open proxy: only the preset XFTP servers of SimpleX's two operators,
//! only on 443, at most `OPEN_MAX` at once per computer, each at most
//! `BYTES_MAX` and `MS_MAX`, and at most `DAY_BYTES_MAX` a day per computer.

use std::collections::BTreeMap;

/// The one port relayed: XFTP's.
pub const PORT: u16 = 443;

/// The hosts relayed: the preset XFTP servers of SimpleX Chat and Flux, the
/// two operators SimpleX's apps and its core (v7.0.3) offer by default. A
/// file a phone sends is on its own servers, these unless its person chose
/// others; one on any other server does not arrive (the bridge says so).
pub const HOSTS: [&str; 12] = [
    "xftp1.simplex.im",
    "xftp2.simplex.im",
    "xftp3.simplex.im",
    "xftp4.simplex.im",
    "xftp5.simplex.im",
    "xftp6.simplex.im",
    "xftp1.simplexonflux.com",
    "xftp2.simplexonflux.com",
    "xftp3.simplexonflux.com",
    "xftp4.simplexonflux.com",
    "xftp5.simplexonflux.com",
    "xftp6.simplexonflux.com",
];

/// Relays open at once per computer, at most: the core receives a file's
/// chunks from a few servers at a time.
pub const OPEN_MAX: usize = 4;
/// Bytes one relay carries, both ways together, at most: a file is at most
/// 25 MiB here (the bridge's attachment bound), a voice note about 1.2 MB.
pub const BYTES_MAX: u64 = 64 * 1024 * 1024;
/// How long one relay stays open, at most; the core opens another as it
/// needs one.
pub const MS_MAX: i64 = 5 * 60 * 1000;
/// Bytes a computer's relays carry in one UTC day, at most.
pub const DAY_BYTES_MAX: u64 = 2 * 1024 * 1024 * 1024;
/// A lease outlives its relay's own bound by this much, then lapses: a
/// relay whose close was never heard frees its place.
pub const LEASE_SLACK_MS: i64 = 30 * 1000;

/// Why a relay is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not one of `HOSTS` on `PORT` (403).
    NotRelayed(String),
    /// `OPEN_MAX` open already (429).
    Busy,
    /// `DAY_BYTES_MAX` carried today already (429).
    Spent,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::NotRelayed(what) => format!("{what} is not relayed: only SimpleX's preset XFTP servers, on port {PORT}"),
            Refusal::Busy => format!("{OPEN_MAX} relays are open already"),
            Refusal::Spent => format!("this computer's relays carried {} GiB today already", DAY_BYTES_MAX >> 30),
        }
    }
}

/// The relay a computer asks for: `host` one of `HOSTS` (any case), `port`
/// `PORT`, as given in the ask's query.
pub fn target(host: &str, port: &str) -> Result<(&'static str, u16), Refusal> {
    let refused = || Refusal::NotRelayed(format!("{}:{}", cut(host), cut(port)));
    let port: u16 = port.parse().map_err(|_| refused())?;
    let host = HOSTS.iter().find(|h| h.eq_ignore_ascii_case(host)).ok_or_else(refused)?;
    if port != PORT {
        return Err(refused());
    }
    Ok((host, port))
}

fn cut(s: &str) -> &str {
    let end = s.char_indices().nth(64).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

/// A computer's relays open now, by lease, each until when it holds its
/// place, and what they carried on its UTC day. Kept in its Durable
/// Object's memory: a restart of the object forgets the open ones, which
/// then lapse on their own bounds (`MS_MAX`).
#[derive(Debug, Default)]
pub struct Leases {
    open: BTreeMap<String, i64>,
    day: u32,
    bytes: u64,
}

impl Leases {
    /// A place for a new relay at `now_ms`, as `lease`, unless `OPEN_MAX`
    /// hold theirs or `DAY_BYTES_MAX` was carried on this UTC day (`spent`,
    /// as kept: its day and bytes). Lapsed leases free theirs first.
    pub fn open(&mut self, lease: &str, now_ms: i64, spent: (u32, u64)) -> Result<(), Refusal> {
        self.restore(spent);
        self.open.retain(|_, until| *until > now_ms);
        if self.day == day_of(now_ms) && self.bytes >= DAY_BYTES_MAX {
            return Err(Refusal::Spent);
        }
        if self.open.len() >= OPEN_MAX {
            return Err(Refusal::Busy);
        }
        assert!(!self.open.contains_key(lease), "a lease is made once");
        self.open.insert(lease.to_string(), now_ms + MS_MAX + LEASE_SLACK_MS);
        Ok(())
    }

    /// `lease`'s relay closed at `now_ms`, having carried `bytes` (at most
    /// `BYTES_MAX` are counted): its place freed, its bytes counted on this
    /// day. Answers the day's tally to keep. Again, or for a lease it does
    /// not hold (one forgotten in a restart), it counts the bytes only once
    /// the lease is its own.
    pub fn close(&mut self, lease: &str, bytes: u64, now_ms: i64, spent: (u32, u64)) -> (u32, u64) {
        self.restore(spent);
        if self.open.remove(lease).is_some() {
            let today = day_of(now_ms);
            if self.day != today {
                (self.day, self.bytes) = (today, 0);
            }
            self.bytes = self.bytes.saturating_add(bytes.min(BYTES_MAX));
        }
        (self.day, self.bytes)
    }

    /// Open now, at `now_ms`.
    pub fn live(&self, now_ms: i64) -> usize {
        self.open.values().filter(|until| **until > now_ms).count()
    }

    /// The day's tally as kept, when it is newer than the one held (a
    /// restart of the object reads it back).
    fn restore(&mut self, (day, bytes): (u32, u64)) {
        if day > self.day || (day == self.day && bytes > self.bytes) {
            (self.day, self.bytes) = (day, bytes);
        }
    }
}

/// The UTC day `ms` falls on, counted from 1970.
pub fn day_of(ms: i64) -> u32 {
    u32::try_from(ms.div_euclid(86_400_000)).expect("a day after 1970")
}

/// The day's tally as `meta` keeps it: `<day> <bytes>`; none, or one that
/// does not read, is nothing carried.
pub fn parse_spent(text: Option<&str>) -> (u32, u64) {
    text.and_then(|t| t.split_once(' ')).and_then(|(d, b)| Some((d.parse().ok()?, b.parse().ok()?))).unwrap_or((0, 0))
}

pub fn format_spent((day, bytes): (u32, u64)) -> String {
    format!("{day} {bytes}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_792_000_000_000;

    /// Goal: only SimpleX's preset XFTP servers, only on 443, are relayed:
    /// any other host, port, or a host dressed up as one (a suffix, a
    /// prefix, a trailing dot, a port in the host), is refused, saying
    /// what. Case does not matter; the host answered is the list's own.
    #[test]
    fn only_the_preset_xftp_servers_on_443_are_relayed() {
        for h in HOSTS {
            assert_eq!(target(h, "443"), Ok((h, 443)));
        }
        assert_eq!(target("XFTP3.SimpleX.im", "443"), Ok(("xftp3.simplex.im", 443)));
        for (host, port) in [
            ("xftp3.simplex.im", "5223"),
            ("xftp3.simplex.im", "80"),
            ("xftp3.simplex.im", "0443x"),
            ("xftp3.simplex.im", ""),
            ("xftp3.simplex.im", "65979"),
            ("smp4.simplex.im", "443"),
            ("xftp7.simplex.im", "443"),
            ("xftp3.simplex.im.evil.com", "443"),
            ("evil-xftp3.simplex.im", "443"),
            ("xftp3.simplex.im.", "443"),
            ("xftp3.simplex.im:443", "443"),
            ("23.128.248.115", "443"),
            ("api.fragment.internal", "443"),
            ("", "443"),
        ] {
            let r = target(host, port);
            assert!(matches!(&r, Err(Refusal::NotRelayed(_))), "{host}:{port}: {r:?}");
        }
        let long = "x".repeat(1000);
        let Err(Refusal::NotRelayed(said)) = target(&long, "443") else { panic!("refused") };
        assert!(said.len() < 80, "what it says of the ask is cut: {said}");
    }

    /// Goal: at most `OPEN_MAX` relays at once per computer; a close frees
    /// its place, and a lease whose close is never heard lapses on its own
    /// bound. Valid, invalid (a full house), replayed (a close twice, a
    /// close of a lease it never held) and restarted (a fresh object,
    /// reading back the day's tally).
    #[test]
    fn a_computer_holds_at_most_its_relays_and_a_lost_close_lapses() {
        let mut l = Leases::default();
        for n in 0..OPEN_MAX {
            l.open(&format!("l{n}"), T, (0, 0)).expect("a place");
        }
        assert_eq!(l.open("full", T, (0, 0)), Err(Refusal::Busy));
        assert_eq!(l.live(T), OPEN_MAX);
        let kept = l.close("l0", 1_000, T + 10, (0, 0));
        assert_eq!(kept, (day_of(T), 1_000));
        assert_eq!(l.close("l0", 1_000, T + 20, kept), kept, "a close again counts nothing more");
        assert_eq!(l.close("never", 5_000, T + 20, kept), kept, "nor one of a lease it never held");
        l.open("l4", T + 30, kept).expect("the freed place");
        assert_eq!(l.open("full", T + 40, kept), Err(Refusal::Busy));
        // the rest never close: they lapse past their bound
        let later = T + 30 + MS_MAX + LEASE_SLACK_MS + 1;
        assert_eq!(l.live(later), 0);
        l.open("after", later, kept).expect("lapsed leases free their places");
        // a restarted object holds none, and reads back the day's tally
        let mut fresh = Leases::default();
        assert_eq!(fresh.close("after", 7, later, kept), kept, "a lease it forgot counts nothing");
        fresh.open("again", later, kept).expect("a place");
        assert_eq!(fresh.close("again", 500, later + 1, kept), (day_of(T), 1_500));
    }

    /// Goal: a computer's relays carry at most `DAY_BYTES_MAX` a UTC day:
    /// past it a new one is refused until the next day; one relay counts at
    /// most `BYTES_MAX`, whatever its close claims.
    #[test]
    fn a_computer_carries_at_most_its_days_bytes() {
        let mut l = Leases::default();
        let mut kept = (0, 0);
        let mut n = 0;
        while kept.1 < DAY_BYTES_MAX {
            l.open(&format!("l{n}"), T, kept).expect("under the day's bound");
            kept = l.close(&format!("l{n}"), u64::MAX, T, kept);
            n += 1;
        }
        assert_eq!(kept.1, DAY_BYTES_MAX, "each counted at its bound");
        assert_eq!(n as u64, DAY_BYTES_MAX / BYTES_MAX);
        assert_eq!(l.open("over", T, kept), Err(Refusal::Spent));
        assert_eq!(Leases::default().open("restarted", T, kept), Err(Refusal::Spent), "a restart reads the tally back");
        let tomorrow = T + 86_400_000;
        l.open("tomorrow", tomorrow, kept).expect("a new day");
        assert_eq!(l.close("tomorrow", 10, tomorrow, kept), (day_of(tomorrow), 10));
        assert_eq!(parse_spent(Some(&format_spent(kept))), kept);
        for bad in [None, Some(""), Some("x y"), Some("12"), Some("1 -2")] {
            assert_eq!(parse_spent(bad), (0, 0), "{bad:?}");
        }
    }
}
