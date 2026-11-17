//! How the platform's notices say amounts and times (FIN-458): one way for
//! every owner's words (`computer::said`), so a person reads $5.03, 3.2 GB
//! and 12 minutes the same wherever they are told.

use crate::price::USD;

/// Dollars as a person reads them: whole ones bare ($20), else to the cent
/// ($5.03), thousands grouped ($1,000.40).
pub fn usd(micros: i64) -> String {
    let sign = if micros < 0 { "-" } else { "" };
    let cents = (micros.unsigned_abs() + 5_000) / 10_000;
    let (whole, frac) = (cents / 100, cents % 100);
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(d);
    }
    match micros.unsigned_abs() % USD.unsigned_abs() {
        0 => format!("{sign}${grouped}"),
        _ => format!("{sign}${grouped}.{frac:02}"),
    }
}

/// Bytes as a person reads them: GB to a tenth, under one in whole MB (at
/// least 1).
pub fn size(bytes: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    if bytes >= GB {
        let tenths = (bytes.saturating_mul(10) + GB / 2) / GB;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", ((bytes + MB / 2) / MB).max(1))
    }
}

/// A span of time as a person reads it, a count whole and rounded down
/// A day as a notice says it, "November 16", on UTC's calendar: what a
/// date a seat ends on reads as (`registry::said::seat_ends`).
pub fn date(ms: i64) -> String {
    let (_, month, day) = crate::cron::civil(ms.div_euclid(24 * 60 * 60 * 1000));
    format!("{} {day}", crate::schedule::MONTH_NAMES[month as usize - 1])
}

/// ("the 40 minutes before it stopped" reads with any of them): "1 minute"
/// under two, then minutes, hours from two, days from two.
pub fn span(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    let (n, unit) = match minutes {
        0..=1 => return "1 minute".to_string(),
        2..=119 => (minutes, "minutes"),
        120..=2879 => (minutes / 60, "hours"),
        _ => (minutes / 1440, "days"),
    };
    format!("{n} {unit}")
}

/// A provider's name for people: `xai` is xAI, `google-places` Google
/// Places (a provider's name is ASCII: `catalog::valid_name`).
pub fn provider_name(provider: &str) -> String {
    match provider {
        "xai" => "xAI".into(),
        "elevenlabs" => "ElevenLabs".into(),
        "openrouter" => "OpenRouter".into(),
        _ => {
            let mut said = provider.replace('-', " ").into_bytes();
            for i in 0..said.len() {
                if i == 0 || said[i - 1] == b' ' {
                    said[i] = said[i].to_ascii_uppercase();
                }
            }
            String::from_utf8_lossy(&said).into_owned()
        }
    }
}

/// The owners' notice lists, each a `NoticeList` as serde_json writes one
/// (`{"notices":[...]}`), as one list in their order (`GET /api/notices`):
/// read no further than their brackets, so the router decodes no notice
/// and encodes none. `None` if one is not such a list.
pub fn joined(owners: &[&[u8]]) -> Option<Vec<u8>> {
    const HEAD: &[u8] = br#"{"notices":["#;
    let mut out = HEAD.to_vec();
    for owner in owners {
        let items = owner.strip_prefix(HEAD)?.strip_suffix(b"]}")?;
        if !items.is_empty() {
            if out.len() > HEAD.len() {
                out.push(b',');
            }
            out.extend_from_slice(items);
        }
    }
    out.extend_from_slice(b"]}");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel, NoticeList};

    /// Goal: the owners' lists, joined as bytes, are the one list a client
    /// reads: every notice of each, in the owners' order, none when none
    /// has any; and an answer that is not a notice list fails the read
    /// rather than passing on as one.
    #[test]
    fn owners_lists_are_joined_as_they_came() {
        let notice = |id: &str, text: &str| Notice {
            id: id.into(),
            kind: "invite".into(),
            level: NoticeLevel::Info,
            title: "Garden plans ]}".into(),
            text: Some(text.into()),
            detail: None,
            until: None,
            actions: vec![NoticeAction::post("Accept", "/api/invites/g/accept", serde_json::json!({}))],
            must_answer: id == "b",
            image: None,
        };
        let list = |n: Vec<Notice>| serde_json::to_vec(&NoticeList { notices: n }).expect("a list serializes");
        let (first, second, none) = (list(vec![notice("a", "[one]")]), list(vec![notice("b", "{two}"), notice("c", "")]), list(vec![]));
        let joined = joined(&[&first, &none, &second]).expect("lists join");
        let read: NoticeList = serde_json::from_slice(&joined).expect("one list");
        assert_eq!(read.notices.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(read.notices[1], notice("b", "{two}"), "each notice as its owner wrote it");
        assert_eq!(joined, list(vec![notice("a", "[one]"), notice("b", "{two}"), notice("c", "")]), "byte for byte the list made at once");
        assert_eq!(super::joined(&[&none, &none]), Some(none.clone()), "none told");
        assert_eq!(super::joined(&[&first, br#"{"error":"deploying"}"#]), None, "an answer not a list is no list");
        assert_eq!(super::joined(&[br#"{"notices":[],"more":1}"#]), None, "nor a list with more beside it");
    }

    #[test]
    fn dollars_sizes_and_spans() {
        assert_eq!(usd(20_000_000), "$20");
        assert_eq!(usd(5_030_000), "$5.03");
        assert_eq!(usd(1_000_000_000), "$1,000");
        assert_eq!(usd(1_000_400_000), "$1,000.40");
        assert_eq!(usd(1_234_567_890_000), "$1,234,567.89");
        assert_eq!(usd(4_999), "$0.00", "less than half a cent");
        assert_eq!(usd(-2_500_000), "-$2.50");
        assert_eq!(size(3_240_662_016), "3.2 GB");
        assert_eq!(size(420_000_000), "420 MB");
        assert_eq!(size(10), "1 MB");
        assert_eq!(span(0), "1 minute");
        assert_eq!(span(119_999), "1 minute");
        assert_eq!(span(12 * 60_000 + 59_000), "12 minutes");
        assert_eq!(span(119 * 60_000), "119 minutes");
        assert_eq!(span(3 * 3_600_000), "3 hours");
        assert_eq!(span(3 * 86_400_000), "3 days");
        assert_eq!(span(-5), "1 minute", "a clock behind says the least");
        assert_eq!(provider_name("elevenlabs"), "ElevenLabs");
        assert_eq!(provider_name("google-places"), "Google Places");
        assert_eq!(provider_name("perplexity"), "Perplexity");
    }
}
