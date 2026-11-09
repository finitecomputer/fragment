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
    if bytes >= 1_000_000_000 {
        let tenths = (bytes + 50_000_000) / 100_000_000;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", ((bytes + 500_000) / 1_000_000).max(1))
    }
}

/// A span of time as a person reads it, whole and rounded down: "a minute"
/// under two, then minutes, hours from two, days from two.
pub fn span(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    let (n, unit) = match minutes {
        0..=1 => return "a minute".to_string(),
        2..=119 => (minutes, "minutes"),
        120..=2879 => (minutes / 60, "hours"),
        _ => (minutes / 1440, "days"),
    };
    format!("{n} {unit}")
}

/// A provider's name for people: `xai` is xAI, `google-places` Google Places.
pub fn provider_name(provider: &str) -> String {
    match provider {
        "xai" => "xAI".into(),
        "elevenlabs" => "ElevenLabs".into(),
        "openrouter" => "OpenRouter".into(),
        _ => provider
            .split('-')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(span(0), "a minute");
        assert_eq!(span(119_999), "a minute");
        assert_eq!(span(12 * 60_000 + 59_000), "12 minutes");
        assert_eq!(span(119 * 60_000), "119 minutes");
        assert_eq!(span(3 * 3_600_000), "3 hours");
        assert_eq!(span(3 * 86_400_000), "3 days");
        assert_eq!(span(-5), "a minute", "a clock behind says the least");
        assert_eq!(provider_name("elevenlabs"), "ElevenLabs");
        assert_eq!(provider_name("google-places"), "Google Places");
        assert_eq!(provider_name("perplexity"), "Perplexity");
    }
}
