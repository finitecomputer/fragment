//! A late card answer's journal read (cards.rs): bounded in pages, bytes
//! and records, and complete or refused: a captured seq window must be
//! contiguous, so a short page is never read as an empty past.
use crate::{api::ApiError, limits, records::Record};

/// What a late card answer's read found (driver.rs `card_history`): `work`
/// back to the card's turn's start, `chat` back to before the card, and
/// that turn's cause when it read.
#[derive(Debug, Clone, PartialEq)]
pub struct Journal {
    pub work: Vec<Record>,
    pub chat: Vec<Record>,
    pub cause: Option<Record>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// Past a bound: a busy chat's card further back than a read goes.
    Budget,
    Gap,
    Corrupt,
    Missing,
    /// No answer, or the platform's own failure (a deploy's 503): read
    /// again from the cursor after a wait, never final.
    Read,
}

impl Unavailable {
    pub fn why(self) -> &'static str {
        match self {
            Self::Budget => "past the read's bounds",
            Self::Gap => "a gap in the journal",
            Self::Corrupt => "a record that is not what the bridge writes",
            Self::Missing => "a record it needs is not there",
            Self::Read => "the journal did not answer",
        }
    }
}

impl From<ApiError> for Unavailable {
    fn from(error: ApiError) -> Self {
        match error {
            e if e.retryable() => Self::Read,
            ApiError::TooLarge => Self::Budget,
            ApiError::Decode(_) => Self::Corrupt,
            _ => Self::Missing,
        }
    }
}

/// One lookup's bounds, shared by its metadata, `work`, `chat` and a cause
/// in another channel.
pub struct Budget {
    pages: usize,
    bytes: usize,
    records: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { pages: limits::HISTORY_PAGES_MAX, bytes: limits::HISTORY_BYTES_MAX, records: limits::HISTORY_RECORDS_MAX }
    }
}

impl Budget {
    /// One call: what its body may be, at most.
    pub fn request(&mut self) -> Result<usize, Unavailable> {
        self.pages = self.pages.checked_sub(1).ok_or(Unavailable::Budget)?;
        if self.bytes == 0 {
            return Err(Unavailable::Budget);
        }
        Ok(self.bytes.min(limits::ANSWER_MAX_BYTES))
    }

    pub fn bytes(&mut self, bytes: usize) -> Result<(), Unavailable> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(Unavailable::Budget)?;
        Ok(())
    }

    /// A page asked for the records after `after`, covering through `upto`
    /// with no gap (newer ones may follow in the same page).
    pub fn page(&mut self, records: &[Record], channel: &str, after: u64, upto: u64) -> Result<(), Unavailable> {
        if records.len() > limits::NOTE_PAGE_RECORDS as usize {
            return Err(Unavailable::Corrupt);
        }
        self.records = self.records.checked_sub(records.len()).ok_or(Unavailable::Budget)?;
        let mut expected = after.checked_add(1).ok_or(Unavailable::Corrupt)?;
        for r in records {
            if r.channel != channel || r.seq != expected {
                return Err(Unavailable::Gap);
            }
            expected = expected.checked_add(1).ok_or(Unavailable::Corrupt)?;
        }
        if expected <= upto {
            return Err(Unavailable::Gap);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(seq: u64) -> Record {
        Record { channel: "work".into(), seq, at: 1, principal: "npub1agent".into(), kind: "message".into(), body: json!({ "kind": "turn.end" }) }
    }

    #[test]
    fn gaps_short_pages_duplicates_and_wrong_channels_are_unavailable() {
        for records in [vec![], vec![rec(1)], vec![rec(1), rec(3)], vec![rec(1), rec(1)], vec![Record { channel: "chat".into(), ..rec(1) }, rec(2)]] {
            assert_eq!(Budget::default().page(&records, "work", 0, 2), Err(Unavailable::Gap));
        }
        assert!(Budget::default().page(&[rec(1), rec(2), rec(3)], "work", 0, 2).is_ok());
    }

    #[test]
    fn each_bound_is_finite_and_a_failed_read_is_read_again() {
        let mut b = Budget::default();
        for _ in 0..limits::HISTORY_PAGES_MAX {
            b.request().unwrap();
        }
        assert_eq!(b.request(), Err(Unavailable::Budget));
        let mut b = Budget::default();
        b.bytes(limits::HISTORY_BYTES_MAX).unwrap();
        assert_eq!(b.request(), Err(Unavailable::Budget));
        let mut b = Budget { records: 0, ..Budget::default() };
        assert_eq!(b.page(&[rec(1)], "work", 0, 1), Err(Unavailable::Budget));
        for (e, want) in [
            (ApiError::Transport("timed out".into()), Unavailable::Read),
            (ApiError::Refused { status: 503, error: "deploying".into(), message: String::new() }, Unavailable::Read),
            (ApiError::Refused { status: 429, error: String::new(), message: String::new() }, Unavailable::Read),
            (ApiError::Refused { status: 404, error: String::new(), message: String::new() }, Unavailable::Missing),
            (ApiError::TooLarge, Unavailable::Budget),
            (ApiError::Decode("x".into()), Unavailable::Corrupt),
        ] {
            assert_eq!(Unavailable::from(e), want);
        }
    }
}
