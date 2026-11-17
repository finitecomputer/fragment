//! Where a job's `fetch` may go. Jobs are author code on a shared fleet,
//! so the platform refuses addresses on the fleet's own networks (loopback,
//! private, link-local, Fly's `.internal` and `.flycast` names) unless the
//! fleet allows local egress (dev and e2e, which call local fakes), or its
//! operator lists the host (`Allow`: a self-hosted org's intranet). The
//! check is on the URL: a public name that resolves to a private address
//! is the fleet's outbound firewall's to stop (on a self-hosted node,
//! celld's `CELLD_EGRESS_PUBLIC_ONLY`, which takes the same list:
//! `Allow::celld`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
}

fn private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    ip.is_loopback() || ip.is_unspecified() || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local (Fly's 6PN is fdaa::/16)
        || (first & 0xffc0) == 0xfe80 // link-local
}

const LOCAL_SUFFIXES: [&str; 5] = ["localhost", "internal", "local", "flycast", "localdomain"];

/// Whether a URL's host is on the fleet's own networks, or names none.
pub fn local(url: &Url) -> bool {
    match url.host() {
        None => true,
        Some(Host::Ipv4(ip)) => private_v4(ip),
        Some(Host::Ipv6(ip)) => private_v6(ip),
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            // a bare name (no dot) resolves inside whatever network the node is on
            !d.contains('.') || LOCAL_SUFFIXES.iter().any(|s| d == *s || d.ends_with(&format!(".{s}"))) || d.parse::<IpAddr>().is_ok()
        }
    }
}

/// The URL a job may fetch, or why not: a public one, or a local one the
/// fleet allows (dev and e2e) or its operator lists (`allow`).
pub fn check(raw: &str, allow_local: bool, allow: &Allow) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("not a URL: {e}"))?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(format!("{}: only http and https", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in the URL are refused; send them in a header".into());
    }
    if url.host().is_none() {
        return Err("the URL has no host".into());
    }
    if local(&url) && !allow_local && !allow.permits(&url) {
        return Err(format!("{} is on a private network; jobs reach public addresses only", url.host_str().unwrap_or("")));
    }
    Ok(url)
}

/// The most entries an operator's egress allowlist (`egress_allow`) holds.
pub const ALLOW_MAX: usize = 64;
/// The longest entry: a DNS name's most (253 bytes) and a glob's `*.`.
pub const ALLOW_ENTRY_MAX_BYTES: usize = 255;

/// One entry of the allowlist, as it matches a URL's host.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// A host name, exactly (`wiki.corp.example`, `intranet`).
    Name(String),
    /// `*.<name>`: one label under the name, and not the name itself.
    Under(String),
    /// An address, or a network (`10.20.0.0/16`), its host bits cleared.
    Net(IpAddr, u8),
}

/// What an allowlist may never open, whatever it lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Never {
    Loopback,
    LinkLocal,
    Unspecified,
    Multicast,
    Broadcast,
    /// `localhost` and the names under it.
    Localhost,
    /// The platform's own host, or its fragments' suffix: a call there
    /// would come back through the front door as no one.
    Platform,
}

impl Never {
    fn what(self) -> &'static str {
        match self {
            Never::Loopback => "loopback",
            Never::LinkLocal => "link-local (the cloud's metadata service among them)",
            Never::Unspecified => "unspecified",
            Never::Multicast => "multicast",
            Never::Broadcast => "broadcast",
            Never::Localhost => "localhost",
            Never::Platform => "the platform's own",
        }
    }
}

/// Why an allowlist is refused (at the deploy, and at the cell's start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowError {
    /// More than `ALLOW_MAX` entries.
    TooMany(usize),
    /// An entry longer than `ALLOW_ENTRY_MAX_BYTES`.
    TooLong(String),
    /// An entry that is no host name, `*.` glob, address or network.
    NotAnEntry(String),
    /// An entry that names, or a network that holds, what is never opened.
    Never(String, Never),
}

impl std::fmt::Display for AllowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AllowError::TooMany(n) => write!(f, "{n} entries, more than {ALLOW_MAX}"),
            AllowError::TooLong(e) => write!(f, "{:?}... is longer than {ALLOW_ENTRY_MAX_BYTES} bytes", e.chars().take(32).collect::<String>()),
            AllowError::NotAnEntry(e) => write!(f, "{e:?} is no host name, `*.` glob, IP address or network (a.b.c.d/n)"),
            AllowError::Never(e, why) => write!(f, "{e:?} is or holds a {} address, which egress never reaches", why.what()),
        }
    }
}

/// An operator's egress allowlist (`FRAGMENT_EGRESS_ALLOW`): the local
/// hosts a job's fetch, a push endpoint and a subscription may reach
/// beside the public internet. Empty, the default, it opens nothing.
/// Whatever it lists, it never opens a `Never` address or name: those
/// are refused when it is read, and a glob or network that covers one
/// (`*.corp.example` over the platform's `fragment.corp.example`) leaves
/// it out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allow {
    entries: Vec<Entry>,
    /// The platform's host and the fragments' suffix, which no entry opens.
    platform: Vec<String>,
}

/// A host name as it is compared: lower case, without a trailing dot.
fn name_of(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Whether `name` is `under` or a name below it.
fn at_or_under(name: &str, under: &str) -> bool {
    name == under || name.strip_suffix(under).is_some_and(|rest| rest.ends_with('.'))
}

/// A host name a URL keeps as a name: labels of letters, digits, `-` and
/// `_`, the last not all digits (`10.1` is an address to a URL).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
        && name.len() <= 253
        && name.split('.').all(|l| (1..=63).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') && !l.starts_with('-') && !l.ends_with('-'))
}

/// An IPv6 address that reaches an IPv4 one (mapped, `::ffff:a.b.c.d`,
/// or NAT64's `64:ff9b::a.b.c.d`), as that address.
fn as_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    ip.to_ipv4_mapped().or_else(|| (s[..6] == [0x64, 0xff9b, 0, 0, 0, 0]).then(|| Ipv4Addr::from((u32::from(s[6]) << 16) | u32::from(s[7]))))
}

fn bits(ip: IpAddr) -> (u128, u8) {
    match ip {
        IpAddr::V4(v4) => (u128::from(u32::from(v4)), 32),
        IpAddr::V6(v6) => (u128::from(v6), 128),
    }
}

/// The network `ip/prefix`, its host bits cleared.
fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
    let (b, width) = bits(ip);
    let keep = if prefix == 0 { 0 } else { u128::MAX << (width - prefix) };
    let b = b & keep;
    match ip {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from(b as u32)),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(b)),
    }
}

/// Whether two networks of one family share an address.
fn overlap((a, a_len): (IpAddr, u8), (b, b_len): (IpAddr, u8)) -> bool {
    let shorter = a_len.min(b_len);
    a.is_ipv4() == b.is_ipv4() && masked(a, shorter) == masked(b, shorter)
}

/// The networks no entry may hold, each with why.
const NEVER_NETS: [(IpAddr, u8, Never); 11] = [
    (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)), 8, Never::Loopback),
    (IpAddr::V4(Ipv4Addr::new(169, 254, 0, 0)), 16, Never::LinkLocal),
    (IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8, Never::Unspecified),
    (IpAddr::V4(Ipv4Addr::new(224, 0, 0, 0)), 4, Never::Multicast),
    (IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 32, Never::Broadcast),
    // `::`, `::1` and the IPv4-compatible `::a.b.c.d`
    (IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)), 96, Never::Loopback),
    (IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 10, Never::LinkLocal),
    (IpAddr::V6(Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0)), 8, Never::Multicast),
    // IPv4 behind IPv6 (mapped, NAT64): listed as IPv4, a network wider
    // than its /96 would hold IPv4's loopback
    (IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0, 0)), 96, Never::Loopback),
    (IpAddr::V6(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0)), 96, Never::Loopback),
    (IpAddr::V6(Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0)), 48, Never::Loopback),
];

/// Why a network holds what is never opened, if it does.
fn never_net(ip: IpAddr, prefix: u8) -> Option<Never> {
    NEVER_NETS.iter().find(|(n, len, _)| overlap((ip, prefix), (*n, *len))).map(|(_, _, why)| *why)
}

/// An address, IPv4 behind IPv6 as the IPv4 it reaches.
fn plain(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => as_v4(v6).map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

impl Allow {
    /// No allowlist: today's rule, public addresses only.
    pub const NONE: Allow = Allow { entries: Vec::new(), platform: Vec::new() };

    /// The allowlist `list` names (host names, one-level `*.` globs, IP
    /// addresses and networks), for a deployment whose platform is at
    /// `platform_host` and whose fragments are under `fragment_suffix`.
    pub fn parse<S: AsRef<str>>(list: &[S], platform_host: &str, fragment_suffix: &str) -> Result<Allow, AllowError> {
        if list.len() > ALLOW_MAX {
            return Err(AllowError::TooMany(list.len()));
        }
        let platform = vec![name_of(platform_host), name_of(fragment_suffix)];
        let mut entries = Vec::with_capacity(list.len());
        for raw in list {
            let raw = raw.as_ref().trim();
            if raw.len() > ALLOW_ENTRY_MAX_BYTES {
                return Err(AllowError::TooLong(raw.into()));
            }
            let never = |why| Err(AllowError::Never(raw.into(), why));
            let not = || AllowError::NotAnEntry(raw.into());
            let entry = if let Some((ip, prefix)) = raw.split_once('/') {
                let ip: IpAddr = ip.parse().map_err(|_| not())?;
                let prefix: u8 = prefix.parse().ok().filter(|p| *p <= bits(ip).1 && !prefix.starts_with('+')).ok_or_else(not)?;
                Entry::Net(ip, prefix)
            } else if let Ok(ip) = raw.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
                Entry::Net(ip, bits(ip).1)
            } else if let Some(under) = raw.strip_prefix("*.") {
                Entry::Under(name_of(under))
            } else {
                Entry::Name(name_of(raw))
            };
            let entry = match entry {
                Entry::Net(IpAddr::V6(v6), prefix) if prefix >= 96 && as_v4(v6).is_some() => {
                    Entry::Net(IpAddr::V4(as_v4(v6).expect("checked")), prefix - 96)
                }
                e => e,
            };
            match &entry {
                Entry::Net(ip, prefix) => {
                    if let Some(why) = never_net(*ip, *prefix) {
                        return never(why);
                    }
                }
                Entry::Name(n) | Entry::Under(n) => {
                    if !valid_name(n) || n.parse::<IpAddr>().is_ok() {
                        return Err(not());
                    }
                    if at_or_under(n, "localhost") {
                        return never(Never::Localhost);
                    }
                    if platform.iter().any(|p| at_or_under(n, p)) {
                        return never(Never::Platform);
                    }
                }
            }
            entries.push(match entry {
                Entry::Net(ip, prefix) => Entry::Net(masked(ip, prefix), prefix),
                e => e,
            });
        }
        Ok(Allow { entries, platform })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether the list opens `url`'s host: one it names, and never one of
    /// the `Never` set.
    pub fn permits(&self, url: &Url) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let ip = match url.host() {
            None => return false,
            Some(Host::Ipv4(v4)) => IpAddr::V4(v4),
            Some(Host::Ipv6(v6)) => plain(IpAddr::V6(v6)),
            Some(Host::Domain(d)) => {
                let d = name_of(d);
                if d.parse::<IpAddr>().is_ok() || at_or_under(&d, "localhost") || self.platform.iter().any(|p| at_or_under(&d, p)) {
                    return false;
                }
                return self.entries.iter().any(|e| match e {
                    Entry::Name(n) => *n == d,
                    Entry::Under(n) => d.strip_suffix(n.as_str()).and_then(|l| l.strip_suffix('.')).is_some_and(|l| !l.is_empty() && !l.contains('.')),
                    Entry::Net(..) => false,
                });
            }
        };
        let width = bits(ip).1;
        never_net(ip, width).is_none() && self.entries.iter().any(|e| matches!(e, Entry::Net(n, len) if overlap((*n, *len), (ip, width))))
    }

    /// The list a Worker variable (`FRAGMENT_EGRESS_ALLOW`) holds, as `var`
    /// wrote it: absent, empty or blank, none (`var` of an empty list is
    /// `""`); else its comma-separated entries, each read by `parse`, so an
    /// empty one among them (`a,,b`, a trailing comma) is refused.
    pub fn from_var(raw: Option<&str>, platform_host: &str, fragment_suffix: &str) -> Result<Allow, AllowError> {
        match raw.map(str::trim).filter(|r| !r.is_empty()) {
            None => Ok(Allow::NONE),
            Some(list) => Allow::parse(&list.split(',').collect::<Vec<_>>(), platform_host, fragment_suffix),
        }
    }

    /// The list as a Worker variable (`FRAGMENT_EGRESS_ALLOW`), comma
    /// separated, as `from_var` reads it back.
    pub fn var<S: AsRef<str>>(list: &[S]) -> String {
        list.iter().map(|e| e.as_ref().trim()).collect::<Vec<_>>().join(",")
    }

    /// The list as a self-hosted node's `CELLD_EGRESS_ALLOW`: each entry
    /// it opens, then the names it never opens, each with a `!` (the
    /// platform's host and the fragments' suffix, each with every name
    /// under it), so the node's resolver leaves them out of a glob as the
    /// cell does. Empty when the list is: a node's public-only rule as
    /// before.
    pub fn celld(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let open = self.entries.iter().map(|e| match e {
            Entry::Name(n) => n.clone(),
            Entry::Under(n) => format!("*.{n}"),
            Entry::Net(ip, len) => format!("{ip}/{len}"),
        });
        open.chain(self.platform.iter().map(|p| format!("!{p}"))).collect::<Vec<_>>().join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::{check, Allow, AllowError, Never, ALLOW_ENTRY_MAX_BYTES, ALLOW_MAX};

    const PUBLIC: [&str; 4] = ["https://api.example.com/v1/chat", "http://example.com:8080/x?y=1", "https://1.1.1.1/", "https://[2606:4700::1111]/"];
    const LOCAL: [&str; 19] = [
        "http://localhost:8790/", "http://127.0.0.1/", "http://10.0.0.1/", "http://192.168.1.1/", "http://172.16.0.1/", "http://169.254.169.254/latest",
        "http://[::1]/", "http://[fdaa:0:1::3]:8080/", "http://[fe80::1]/", "http://[::ffff:127.0.0.1]/", "http://0.0.0.0/", "http://100.64.0.1/",
        "http://my-app.internal/", "http://my-app.flycast/", "http://a.fragment.localhost:8790/", "http://intranet/", "http://[::ffff:10.0.0.1]/",
        "http://wiki.corp.internal./", "http://224.0.0.1/",
    ];

    fn allow(list: &[&str]) -> Allow {
        Allow::parse(list, "fragment.corp.example", "apps.corp.example").unwrap_or_else(|e| panic!("{list:?}: {e}"))
    }

    fn refused(list: &[&str]) -> AllowError {
        Allow::parse(list, "fragment.corp.example", "apps.corp.example").expect_err(&format!("{list:?}"))
    }

    #[test]
    fn public_only() {
        for ok in PUBLIC {
            assert!(check(ok, false, &Allow::NONE).is_ok(), "{ok}");
        }
        for bad in LOCAL.iter().copied().chain(["ftp://example.com/", "file:///etc/passwd", "https://user:pw@example.com/", "not a url"]) {
            assert!(check(bad, false, &Allow::NONE).is_err(), "{bad}");
        }
        assert!(check("http://127.0.0.1:9999/", true, &Allow::NONE).is_ok(), "dev and e2e allow local egress");
    }

    /// `check` as master had it before the allowlist (f3f82ee8d), verbatim
    /// but for its name: what an empty list must still answer.
    fn master_check(raw: &str, allow_local: bool) -> Result<url::Url, String> {
        let url = url::Url::parse(raw).map_err(|e| format!("not a URL: {e}"))?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err(format!("{}: only http and https", url.scheme()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("credentials in the URL are refused; send them in a header".into());
        }
        if url.host().is_none() {
            return Err("the URL has no host".into());
        }
        if super::local(&url) && !allow_local {
            return Err(format!("{} is on a private network; jobs reach public addresses only", url.host_str().unwrap_or("")));
        }
        Ok(url)
    }

    // An empty list, however a deployment arrives at it (no variable, an
    // empty or blank one, `var` of an empty list), answers every URL as
    // master did, error for error: against master's own `check`, and
    // against a table of master's verdicts written out.
    #[test]
    fn an_empty_list_is_today() {
        assert_eq!(Allow::var::<&str>(&[]), "");
        let empties = [None, Some(""), Some("   "), Some(Allow::var::<&str>(&[]).as_str())]
            .map(|raw| Allow::from_var(raw, "fragment.corp.example", "apps.corp.example").expect("empty is no list"));
        let extra = ["http://wiki.corp.example/", "http://fragment.corp.example/", "https://x.apps.corp.example/", "http://wiki.internal/", "http://10.20.0.1/"];
        for empty in empties.iter().chain([&allow(&[]), &Allow::NONE]) {
            assert!(empty.is_empty() && empty.celld().is_empty());
            for raw in PUBLIC.iter().chain(LOCAL.iter()).chain(extra.iter()).copied().chain(["ftp://x.example/", "https://u:p@x.example/", "not a url"]) {
                for local in [false, true] {
                    assert_eq!(check(raw, local, empty), master_check(raw, local), "{raw}");
                }
            }
        }
        let empty = &empties[1];
        for (raw, verdict) in [
            ("https://api.example.com/v1/chat", Ok("https://api.example.com/v1/chat")),
            ("http://wiki.corp.example/", Ok("http://wiki.corp.example/")),
            ("http://10.20.0.1/", Err("10.20.0.1 is on a private network; jobs reach public addresses only")),
            ("http://100.101.102.103/", Err("100.101.102.103 is on a private network; jobs reach public addresses only")),
            ("http://wiki.internal/", Err("wiki.internal is on a private network; jobs reach public addresses only")),
            ("http://intranet/", Err("intranet is on a private network; jobs reach public addresses only")),
            ("http://169.254.169.254/latest", Err("169.254.169.254 is on a private network; jobs reach public addresses only")),
            ("http://localhost:8790/", Err("localhost is on a private network; jobs reach public addresses only")),
            ("https://u:p@x.example/", Err("credentials in the URL are refused; send them in a header")),
            ("ftp://x.example/", Err("ftp: only http and https")),
        ] {
            assert_eq!(check(raw, false, empty).map(String::from).as_deref().map_err(String::as_str), verdict, "{raw}");
        }
        assert!(check("http://10.20.0.1/", true, empty).is_ok(), "dev and e2e allow local egress, as before");
    }

    // Valid: a listed name, glob, address or network opens its local hosts,
    // whatever their case, trailing dot or port, and nothing beside them.
    #[test]
    fn listed_hosts_pass() {
        let a = allow(&["wiki.internal", "*.corp.internal", "10.20.0.0/16", "100.101.102.103", "fd7a:115c:a1e0::/48", " Intranet. "]);
        for ok in [
            "http://wiki.internal/", "https://WIKI.Internal.:8443/x", "http://git.corp.internal/", "http://GIT.corp.internal./",
            "http://10.20.3.4:8080/", "http://[::ffff:10.20.3.4]/", "http://100.101.102.103/", "http://[fd7a:115c:a1e0::5]/", "http://intranet/",
        ] {
            assert!(check(ok, false, &a).is_ok(), "{ok}");
        }
        for bad in [
            "http://wiki2.internal/", "http://corp.internal/", "http://a.git.corp.internal/", "http://10.21.0.1/", "http://100.101.102.104/",
            "http://[fd7a:115c:a1e1::5]/", "http://192.168.1.1/", "http://intranet2/",
        ] {
            assert!(check(bad, false, &a).is_err(), "{bad}");
        }
        // the rest of the check is the check's as before
        assert!(check("https://user:pw@wiki.internal/", false, &a).is_err());
        assert!(check("ftp://wiki.internal/", false, &a).is_err());
        for ok in PUBLIC {
            assert!(check(ok, false, &a).is_ok(), "{ok}");
        }
    }

    // Edges: an entry's IPv4 behind IPv6 is the IPv4 network; its host bits
    // are cleared; an IPv6 literal may be bracketed.
    #[test]
    fn entries_read_as_networks() {
        let a = allow(&["::ffff:10.30.0.0/112", "10.40.1.1/16", "[fd00::7]"]);
        for ok in ["http://10.30.0.9/", "http://10.40.200.1/", "http://[fd00::7]/"] {
            assert!(check(ok, false, &a).is_ok(), "{ok}");
        }
        assert_eq!(a.celld(), "10.30.0.0/16,10.40.0.0/16,fd00::7/128,!fragment.corp.example,!apps.corp.example");
    }

    // Invalid: entries out of shape, and lists past their bounds.
    #[test]
    fn malformed_entries_are_refused() {
        for bad in [
            "", "*", "*.", "*.*.corp", "a.*.corp", "wiki.corp:8080", "http://wiki.corp/", "wiki corp", "-wiki.corp", "wiki..corp", "10.1",
            "10.0.0.0/33", "10.0.0.0/", "10.0.0.0/+8", "fd00::/129", "wiki.corp/16", "bücher.corp",
        ] {
            assert_eq!(refused(&[bad]), AllowError::NotAnEntry(bad.trim().into()), "{bad}");
        }
        let long = format!("{}.corp", "a".repeat(ALLOW_ENTRY_MAX_BYTES));
        assert_eq!(refused(&[long.as_str()]), AllowError::TooLong(long.clone()));
        let many = vec!["wiki.corp"; ALLOW_MAX + 1];
        assert_eq!(refused(&many), AllowError::TooMany(ALLOW_MAX + 1));
        allow(&vec!["wiki.corp"; ALLOW_MAX]);
    }

    // The never set: refused when listed, alone or inside a wider network.
    #[test]
    fn the_never_set_is_refused_when_listed() {
        for (entry, why) in [
            ("127.0.0.1", Never::Loopback),
            ("127.0.0.0/8", Never::Loopback),
            ("0.0.0.0/0", Never::Loopback),
            ("96.0.0.0/3", Never::Loopback),
            ("::1", Never::Loopback),
            ("::/0", Never::Loopback),
            ("::ffff:127.0.0.1", Never::Loopback),
            ("::ffff:0:0/96", Never::Loopback),
            ("64:ff9b::7f00:1", Never::Loopback),
            ("64:ff9b::/96", Never::Loopback),
            ("169.254.169.254", Never::LinkLocal),
            ("169.0.0.0/8", Never::LinkLocal),
            ("fe80::1", Never::LinkLocal),
            ("0.0.0.0", Never::Unspecified),
            ("::", Never::Loopback),
            ("224.0.0.1", Never::Multicast),
            ("ff02::1", Never::Multicast),
            ("255.255.255.255", Never::Broadcast),
            ("localhost", Never::Localhost),
            ("LOCALHOST.", Never::Localhost),
            ("a.fragment.localhost", Never::Localhost),
            ("*.localhost", Never::Localhost),
            ("fragment.corp.example", Never::Platform),
            ("apps.corp.example", Never::Platform),
            ("*.apps.corp.example", Never::Platform),
            ("notes.apps.corp.example", Never::Platform),
        ] {
            assert_eq!(refused(&["wiki.corp", entry]), AllowError::Never(entry.into(), why), "{entry}");
        }
        assert!(refused(&["127.0.0.1"]).to_string().contains("loopback"));
    }

    // The never set stays shut even where an entry covers it: a glob over
    // the platform's host (here under `.internal`, which the URL check
    // judges; a public-looking one is the node's resolver's, which
    // `celld()` hands the names), a mapped spelling, a name for loopback.
    #[test]
    fn the_never_set_stays_shut() {
        let a = Allow::parse(&["*.corp.internal", "10.0.0.0/8", "*.internal"], "fragment.corp.internal", "apps.corp.internal").expect("valid");
        for bad in [
            "http://fragment.corp.internal/", "https://FRAGMENT.corp.internal./", "http://x.apps.corp.internal/", "http://apps.corp.internal/",
            "http://localhost/", "http://127.0.0.1/", "http://[::ffff:127.0.0.1]/", "http://169.254.169.254/", "http://[::1]/", "http://0.0.0.0/",
            "http://a.fragment.localhost/",
        ] {
            assert!(check(bad, false, &a).is_err(), "{bad}");
        }
        for ok in ["http://wiki.corp.internal/", "http://my-app.internal/", "http://10.1.2.3/", "http://[::ffff:10.1.2.3]/"] {
            assert!(check(ok, false, &a).is_ok(), "{ok}");
        }
        assert_eq!(a.celld(), "*.corp.internal,10.0.0.0/8,*.internal,!fragment.corp.internal,!apps.corp.internal");
    }

    // The Worker variable, as the cell reads it at its start (config.rs
    // hands `from_var` the variable's raw value): what `var` writes reads
    // back as the list; an empty entry among others is refused.
    #[test]
    fn the_variable_reads_back() {
        let list = [" wiki.internal ", "10.20.0.0/16"];
        let var = Allow::var(&list);
        assert_eq!(var, "wiki.internal,10.20.0.0/16");
        let read = |raw: &str| Allow::from_var(Some(raw), "fragment.corp.example", "apps.corp.example");
        assert_eq!(read(&var), Ok(allow(&list)));
        assert_eq!(read(&format!(" {var}\n")), Ok(allow(&list)), "a variable's surrounding blanks");
        for bad in ["wiki.internal,,10.20.0.0/16", "wiki.internal,", ",wiki.internal", ",", "wiki.internal, ,10.20.0.0/16"] {
            assert_eq!(read(bad), Err(AllowError::NotAnEntry(String::new())), "{bad:?}");
        }
        assert_eq!(read("wiki.internal,127.0.0.1"), Err(AllowError::Never("127.0.0.1".into(), Never::Loopback)));
    }
}
