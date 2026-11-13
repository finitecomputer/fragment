//! Unix seconds and RFC 3339, the two forms collateral dates come in.

use chrono::{DateTime, SecondsFormat, Utc};

pub fn now() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map_or(0, |d| d.as_secs() as i64)
}

pub fn parse_rfc3339(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text).ok().map(|t| t.timestamp())
}

pub fn rfc3339(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| unix.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips() {
        let t = super::parse_rfc3339("2026-11-10T17:00:00Z").unwrap();
        assert_eq!(super::rfc3339(t), "2026-11-10T17:00:00Z");
        assert_eq!(super::parse_rfc3339("yesterday"), None);
    }
}
