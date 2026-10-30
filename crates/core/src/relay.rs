//! The XFTP relay (docs/computers.md, "The XFTP relay"): raw TCP from a
//! computer's SimpleX core to SimpleX's file servers, through the platform.
//!
//! Cloudflare Containers reset a TLS hello on port 443 that names no server
//! (SNI), and a SimpleX server on 443 takes its protocol only from such a
//! hello, so a computer reaches no XFTP server directly (measured on a
//! preview computer, 2026-10-25; docs/bridge.md, "SimpleX"). A Worker's `connect()` is raw TCP that nothing inspects: the
//! computer's bridge hands the platform the core's bytes over a WebSocket
//! (`GET /api/computer/relay?host=&port=`), and the platform pipes them to
//! the server untouched. The core's TLS runs end to end, checked against the
//! server's own certificate fingerprint, and XFTP encrypts each file's
//! chunks besides: the platform sees the host, the port, the byte counts and
//! the times, never what is sent.
//!
//! No open proxy: only the preset XFTP servers of SimpleX's two operators,
//! only on 443, at most `OPEN_MAX` at once per computer, each at most
//! `BYTES_MAX` and `MS_MAX`, and at most `DAY_BYTES_MAX` a day per computer,
//! each relay's allowance reserved against that as it opens.

use std::collections::BTreeMap;

/// The one port relayed: XFTP's.
pub const PORT: u16 = 443;

/// The guest's route for a relay.
pub const ROUTE: &str = "/api/computer/relay";

/// What tells the shim's `ComputerEgress` that the platform holds a relay's
/// place, by its lease (with `-host`, `-port`, `-bytes`, `-ms`). Only the
/// relay route's own answer carries it: every other answer is stripped of
/// the family (`is_relay_header`) before the shim reads it, and the shim
/// pipes only an answer to the relay route's own upgrade.
pub const HEADER: &str = "x-fragment-relay";

/// Whether a header's name is of `HEADER`'s family (any case).
pub fn is_relay_header(name: &str) -> bool {
    name.get(..HEADER.len()).is_some_and(|p| p.eq_ignore_ascii_case(HEADER))
}

/// Relays open at once per computer, at most: the core receives a file's
/// chunks from a few servers at a time.
pub const OPEN_MAX: usize = 4;
/// Bytes one relay carries, both ways together, at most: a file is at most
/// 25 MiB here (the bridge's attachment bound), a voice note about 1.2 MB.
/// Toward the computer the shim cannot see a slow reader (a Worker's
/// WebSocket shows no backlog), so this, times `OPEN_MAX`, also bounds what
/// one computer's relays can hold in the Worker's memory.
pub const BYTES_MAX: u64 = 32 * 1024 * 1024;
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
    /// Not an exact configured target (403).
    NotRelayed(String),
    /// `OPEN_MAX` open already (429).
    Busy,
    /// `DAY_BYTES_MAX` carried today already (429).
    Spent,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::NotRelayed(what) => format!("{what} is not relayed: only configured connector relay targets"),
            Refusal::Busy => format!("{OPEN_MAX} relays are open already"),
            Refusal::Spent => format!("this computer's relays carried {} GiB today already", DAY_BYTES_MAX >> 30),
        }
    }
}

/// An exact operator-configured endpoint; case-insensitive DNS, no suffix matching.
pub fn target<'a>(policy: &'a crate::connectors::ComputerConnectorPolicy, host: &str, port: &str) -> Result<(&'a str, u16), Refusal> {
    let refused = || Refusal::NotRelayed(format!("{}:{}", cut(host), cut(port)));
    let port: u16 = port.parse().map_err(|_| refused())?;
    policy.targets().find(|t| t.host.eq_ignore_ascii_case(host) && t.port == port).map(|t| (t.host.as_str(), t.port)).ok_or_else(refused)
}

fn cut(s: &str) -> &str {
    let end = s.char_indices().nth(64).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

/// A computer's relays open now, by lease, each with until when it holds
/// its place and the bytes it was allowed, and what they carried on its UTC
/// day. Each lease's allowance is reserved against the day's budget as it
/// is made (Alex's review), so relays open at once never carry past
/// `DAY_BYTES_MAX` between them. Kept in its Durable Object's memory: a
/// restart of the object forgets the open ones (and their reservations),
/// which then lapse on their own bounds (`MS_MAX`).
#[derive(Debug, Default)]
pub struct Leases {
    open: BTreeMap<String, Lease>,
    day: u32,
    bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct Lease {
    until: i64,
    allowance: u64,
}

/// A lease's allowance is at least this, or none is made: a relay too
/// short to carry a chunk of a file only fails.
pub const ALLOWANCE_MIN: u64 = 256 * 1024;

impl Leases {
    /// A place for a new relay at `now_ms`, as `lease`: the bytes it may
    /// carry (`BYTES_MAX`, or what is left of the UTC day's budget past
    /// what it carried and what the open ones hold, if less), unless
    /// `OPEN_MAX` hold theirs, or less than `ALLOWANCE_MIN` is left
    /// (`spent`, as kept: its day and bytes). A lapsed lease (its close
    /// never heard) frees its place first, and counts all it was allowed.
    pub fn open(&mut self, lease: &str, now_ms: i64, spent: (u32, u64)) -> Result<u64, Refusal> {
        self.restore(spent);
        self.lapse(now_ms);
        self.roll(now_ms);
        let held: u64 = self.open.values().map(|l| l.allowance).sum();
        let left = DAY_BYTES_MAX.saturating_sub(self.bytes).saturating_sub(held);
        if left < ALLOWANCE_MIN {
            return Err(Refusal::Spent);
        }
        if self.open.len() >= OPEN_MAX {
            return Err(Refusal::Busy);
        }
        assert!(!self.open.contains_key(lease), "a lease is made once");
        let allowance = left.min(BYTES_MAX);
        self.open.insert(lease.to_string(), Lease { until: now_ms + MS_MAX + LEASE_SLACK_MS, allowance });
        Ok(allowance)
    }

    /// `lease`'s relay closed at `now_ms`, having carried `bytes` (at most
    /// its allowance is counted): its place and reservation freed, its bytes
    /// counted on this day. Again, or for a lease it does not hold (one
    /// forgotten in a restart, or lapsed), it counts nothing.
    pub fn close(&mut self, lease: &str, bytes: u64, now_ms: i64, spent: (u32, u64)) {
        self.restore(spent);
        self.lapse(now_ms);
        if let Some(l) = self.open.remove(lease) {
            self.roll(now_ms);
            self.bytes = self.bytes.saturating_add(bytes.min(l.allowance));
        }
    }

    /// The UTC day's tally to keep: its day and the bytes carried on it.
    pub fn spent(&self) -> (u32, u64) {
        (self.day, self.bytes)
    }

    /// Open now, at `now_ms`.
    pub fn live(&self, now_ms: i64) -> usize {
        self.open.values().filter(|l| l.until > now_ms).count()
    }

    /// Leases past their time: gone, each counting all it was allowed (what
    /// it carried was never told).
    fn lapse(&mut self, now_ms: i64) {
        let lapsed: u64 = self.open.values().filter(|l| l.until <= now_ms).map(|l| l.allowance).sum();
        if lapsed > 0 {
            self.open.retain(|_, l| l.until > now_ms);
            self.roll(now_ms);
            self.bytes = self.bytes.saturating_add(lapsed);
        }
    }

    /// A new UTC day starts its tally at nothing (the open ones' reservations
    /// are held against it).
    fn roll(&mut self, now_ms: i64) {
        let today = day_of(now_ms);
        if self.day != today {
            (self.day, self.bytes) = (today, 0);
        }
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

    /// Goal: every header that could tell the shim to pipe is known as one
    /// (the lease's and its host's, port's, bounds', in any case), and no
    /// other: what is stripped from every answer but the relay route's own.
    #[test]
    fn the_relays_headers_are_known_as_such() {
        for h in ["x-fragment-relay", "x-fragment-relay-host", "X-Fragment-Relay-Port", "x-fragment-relay-bytes", "x-fragment-relay-ms", "x-fragment-relayx"] {
            assert!(is_relay_header(h), "{h}");
        }
        for h in ["x-fragment-rela", "x-fragment-agent", "content-type", "", "relay", "x-fragment-relay"[1..].as_ref()] {
            assert!(!is_relay_header(h), "{h}");
        }
    }

    /// Goal: only SimpleX's preset XFTP servers, only on 443, are relayed:
    /// any other host, port, or a host dressed up as one (a suffix, a
    /// prefix, a trailing dot, a port in the host), is refused, saying
    /// what. Case does not matter; the host answered is the list's own.
    #[test]
    fn only_the_preset_xftp_servers_on_443_are_relayed() {
        let policy = crate::connectors::ComputerConnectorPolicy::default();
        for t in policy.targets() {
            let h = t.host.as_str();
            assert_eq!(target(&policy, h, "443"), Ok((h, 443)));
        }
        assert_eq!(target(&policy, "XFTP3.SimpleX.im", "443"), Ok(("xftp3.simplex.im", 443)));
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
            let r = target(&policy, host, port);
            assert!(matches!(&r, Err(Refusal::NotRelayed(_))), "{host}:{port}: {r:?}");
        }
        let long = "x".repeat(1000);
        let Err(Refusal::NotRelayed(said)) = target(&policy, &long, "443") else { panic!("refused") };
        assert!(said.len() < 80, "what it says of the ask is cut: {said}");
    }

    /// Goal: at most `OPEN_MAX` relays at once per computer; a close frees
    /// its place, and a lease whose close is never heard lapses on its own
    /// bound, counting all it was allowed. Valid, invalid (a full house),
    /// replayed (a close twice, a close of a lease it never held) and
    /// restarted (a fresh object, reading back the day's tally).
    #[test]
    fn a_computer_holds_at_most_its_relays_and_a_lost_close_lapses() {
        let mut l = Leases::default();
        for n in 0..OPEN_MAX {
            assert_eq!(l.open(&format!("l{n}"), T, (0, 0)), Ok(BYTES_MAX), "a place, and all one relay may carry");
        }
        assert_eq!(l.open("full", T, (0, 0)), Err(Refusal::Busy));
        assert_eq!(l.live(T), OPEN_MAX);
        l.close("l0", 1_000, T + 10, (0, 0));
        let kept = l.spent();
        assert_eq!(kept, (day_of(T), 1_000));
        l.close("l0", 1_000, T + 20, kept);
        assert_eq!(l.spent(), kept, "a close again counts nothing more");
        l.close("never", 5_000, T + 20, kept);
        assert_eq!(l.spent(), kept, "nor one of a lease it never held");
        l.open("l4", T + 30, kept).expect("the freed place");
        assert_eq!(l.open("full", T + 40, kept), Err(Refusal::Busy));
        // the rest never close: they lapse past their bound, each counting all it was allowed
        let later = T + 30 + MS_MAX + LEASE_SLACK_MS + 1;
        assert_eq!(l.live(later), 0);
        l.open("after", later, kept).expect("lapsed leases free their places");
        assert_eq!(l.spent(), (day_of(T), 1_000 + 4 * BYTES_MAX), "the four lapsed, at their allowances");
        // a restarted object holds none, and reads back the day's tally
        let kept = l.spent();
        let mut fresh = Leases::default();
        fresh.close("after", 7, later, kept);
        assert_eq!(fresh.spent(), kept, "a lease it forgot counts nothing");
        fresh.open("again", later, kept).expect("a place");
        fresh.close("again", 500, later + 1, kept);
        assert_eq!(fresh.spent(), (day_of(T), kept.1 + 500));
    }

    /// Goal: a computer's relays carry at most `DAY_BYTES_MAX` a UTC day,
    /// however many are open at once (Alex's review): each lease reserves
    /// its allowance as it is made, so near the bound the relays opened
    /// together share what is left and no more, even when each carries all
    /// it may (and claims more); a short relay's unused reservation is the
    /// next one's; past the bound one is refused until the next day, which
    /// starts afresh (a lease lapsing then counts on it); a restart reads
    /// the day's tally back.
    #[test]
    fn a_computer_carries_at_most_its_days_bytes() {
        let nearly = (day_of(T), DAY_BYTES_MAX - 3 * BYTES_MAX / 2);
        let mut l = Leases::default();
        assert_eq!(l.open("a", T, nearly), Ok(BYTES_MAX));
        assert_eq!(l.open("b", T, nearly), Ok(BYTES_MAX / 2), "the rest of the day's budget, past what a holds");
        assert_eq!(l.open("c", T, nearly), Err(Refusal::Spent), "nothing left past what a and b hold");
        l.close("a", u64::MAX, T + 1, nearly);
        l.close("b", u64::MAX, T + 1, nearly);
        assert_eq!(l.spent(), (day_of(T), DAY_BYTES_MAX), "each counted at its allowance: at the bound, never past it");
        assert_eq!(l.open("over", T + 2, l.spent()), Err(Refusal::Spent));
        assert_eq!(Leases::default().open("restarted", T + 2, l.spent()), Err(Refusal::Spent), "a restart reads the tally back");
        // a short relay frees what it did not use
        let mut m = Leases::default();
        m.open("a", T, nearly).expect("a");
        m.open("b", T, nearly).expect("b");
        m.close("a", 1_000, T + 1, nearly);
        assert_eq!(m.open("c", T + 2, m.spent()), Ok(BYTES_MAX - 1_000), "a's unused reservation, less b's");
        // a new day: a fresh budget; a lease that lapses then counts on it
        let mut d = Leases::default();
        assert_eq!(d.open("x", T, (day_of(T), DAY_BYTES_MAX - BYTES_MAX)), Ok(BYTES_MAX));
        assert_eq!(d.open("y", T, d.spent()), Err(Refusal::Spent));
        let tomorrow = T + 86_400_000;
        assert_eq!(d.open("y", tomorrow, d.spent()), Ok(BYTES_MAX), "a new day");
        assert_eq!(d.spent(), (day_of(tomorrow), BYTES_MAX), "x, lapsed, counts all it was allowed, on the day it lapsed");
        assert_eq!(parse_spent(Some(&format_spent(d.spent()))), d.spent());
        for bad in [None, Some(""), Some("x y"), Some("12"), Some("1 -2")] {
            assert_eq!(parse_spent(bad), (0, 0), "{bad:?}");
        }
    }
}
