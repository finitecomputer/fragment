//! The box's zone, and where a host name stands in it.
//!
//! The platform is the zone's apex (`https://<zone>`), and each fragment
//! is one label under it (`https://<name>.<zone>`), its own origin. Nothing
//! deeper is served: the zone's certificate names the apex and `*.<zone>`,
//! and a wildcard covers one label alone, so a deeper name answers nothing
//! (DNS) and is refused (the door).

use std::fmt;

/// A zone's name at most: `<label>.<zone>` (a label is 63 bytes at most)
/// stays within DNS's 253.
pub const ZONE_BYTES_MAX: usize = 253 - 64;
/// A label at most (RFC 1035).
pub const LABEL_BYTES_MAX: usize = 63;

/// A zone: two or more lower-case DNS labels of letters, digits and inner
/// hyphens, `ZONE_BYTES_MAX` bytes at most.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0:?} is not a zone: two or more lower-case DNS labels (letters, digits, inner hyphens), {ZONE_BYTES_MAX} bytes at most, no final dot")]
pub struct ZoneError(pub String);

/// Where a host name stands in the zone.
#[derive(Debug, PartialEq, Eq)]
pub enum Place {
    /// The zone itself: the platform.
    Apex,
    /// One label under it: a fragment's own origin (or no one's yet).
    Under(String),
    /// Two or more labels under it: nothing is served there.
    Deeper,
    /// Not the zone's.
    Outside,
}

impl Zone {
    pub fn parse(name: &str) -> Result<Zone, ZoneError> {
        let labels: Vec<&str> = name.split('.').collect();
        let ok = name.len() <= ZONE_BYTES_MAX && labels.len() >= 2 && labels.iter().all(|l| valid_label(l));
        if ok {
            Ok(Zone(name.to_string()))
        } else {
            Err(ZoneError(name.to_string()))
        }
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// Where `name` (a host name, any case, a final dot or none, no port)
    /// stands in the zone.
    pub fn place(&self, name: &str) -> Place {
        let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
        if name == self.0 {
            return Place::Apex;
        }
        let Some(rest) = name.strip_suffix(self.0.as_str()).and_then(|r| r.strip_suffix('.')) else {
            return Place::Outside;
        };
        match rest.split_once('.') {
            None if valid_label(rest) => Place::Under(rest.to_string()),
            // an empty or unprintable label, or one too long: no host's
            None => Place::Outside,
            Some(_) => Place::Deeper,
        }
    }
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One lower-case DNS label: 1 to 63 bytes of letters, digits and inner
/// hyphens.
pub fn valid_label(l: &str) -> bool {
    (1..=LABEL_BYTES_MAX).contains(&l.len())
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !l.starts_with('-')
        && !l.ends_with('-')
}

/// A `Host` header's name, without its port: `None` for an IP literal or
/// a header that is no host.
pub fn host_name(host: &str) -> Option<&str> {
    if host.starts_with('[') {
        return None;
    }
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        Some(_) => return None,
        None => host,
    };
    let is_address = name.parse::<std::net::IpAddr>().is_ok();
    (!name.is_empty() && !is_address).then_some(name)
}

/// The https origin of `host` behind the door's port (no port for 443).
pub fn https_origin(host: &str, port: u16) -> String {
    match port {
        443 => format!("https://{host}"),
        p => format!("https://{host}:{p}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a zone is lower-case labels, two or more, short enough for a
    /// label under it; anything else is refused.
    #[test]
    fn zones_are_lower_case_dns_names() {
        for ok in ["fragment.home.arpa", "a.b", "x-1.corp.example"] {
            assert_eq!(Zone::parse(ok).map(|z| z.to_string()), Ok(ok.to_string()));
        }
        let long = format!("{}.b", "a".repeat(ZONE_BYTES_MAX));
        for bad in ["arpa", "Fragment.home.arpa", "-a.b", "a-.b", "a..b", "a.b.", "*.a.b", "a_b.c", "", &long] {
            assert!(Zone::parse(bad).is_err(), "{bad}");
        }
        assert!(Zone::parse(&format!("{}.b", "a".repeat(64))).is_err(), "a label past 63 bytes");
    }

    /// Goal: the apex is the platform's, one label under it a fragment's,
    /// deeper names and look-alikes no one's.
    #[test]
    fn hosts_stand_in_the_zone() {
        let z = Zone::parse("fragment.home.arpa").unwrap();
        assert_eq!(z.place("fragment.home.arpa"), Place::Apex);
        assert_eq!(z.place("FRAGMENT.Home.Arpa."), Place::Apex);
        assert_eq!(z.place("todo--k8ew.fragment.home.arpa"), Place::Under("todo--k8ew".into()));
        assert_eq!(z.place("a.b.fragment.home.arpa"), Place::Deeper);
        for outside in ["evilfragment.home.arpa", ".fragment.home.arpa", "home.arpa", "fragment.home.arpa.evil.example", "a_b.fragment.home.arpa", "example.com"] {
            assert_eq!(z.place(outside), Place::Outside, "{outside}");
        }
    }

    #[test]
    fn a_host_header_loses_its_port() {
        assert_eq!(host_name("fragment.home.arpa:8443"), Some("fragment.home.arpa"));
        assert_eq!(host_name("fragment.home.arpa"), Some("fragment.home.arpa"));
        assert_eq!(host_name("127.0.0.1:80"), None, "an address names no host");
        assert_eq!(host_name("[::1]:443"), None);
        assert_eq!(host_name("a.b:x"), None);
        assert_eq!(host_name(""), None);
        assert_eq!(https_origin("fragment.home.arpa", 443), "https://fragment.home.arpa");
        assert_eq!(https_origin("fragment.home.arpa", 8443), "https://fragment.home.arpa:8443");
    }
}
