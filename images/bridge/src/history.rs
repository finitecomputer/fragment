//! Recovery reads are finite and complete or explicitly unavailable. A
//! captured seq window must be contiguous; a short page is not an empty past.
use crate::{api::ApiError, limits, records::Record};

pub const VISIBLE: &str = "History unavailable: required context is missing, corrupt, or exceeds the recovery budget. Earlier actions may already have happened. Ask again with the context; this answer has not resumed earlier work.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable { Budget, Gap, Corrupt, Missing, Read }

impl Unavailable {
    pub fn message(self) -> String {
        let why = match self {
            Self::Budget => "the recovery budget was exceeded",
            Self::Gap => "the journal has a gap or truncated page",
            Self::Corrupt => "the required journal context is corrupt",
            Self::Missing => "the required journal context is missing",
            Self::Read => "the journal could not be read in time",
        };
        format!("History unavailable: {why}. Earlier actions may already have happened; ask for context before repeating them.")
    }
}
impl From<ApiError> for Unavailable {
    fn from(error: ApiError) -> Self {
        match error { ApiError::TooLarge => Self::Budget, ApiError::Decode(_) => Self::Corrupt, _ => Self::Read }
    }
}

/// Shared across metadata, work, chat and a cross-channel cause lookup.
/// Work is charged pessimistically for each predicate's possible quadratic
/// scan (P5's previous-turn search), plus linear validation/rendering.
pub struct Budget { pages: usize, bytes: usize, records: usize, work: usize }
impl Default for Budget {
    fn default() -> Self {
        Self { pages: limits::HISTORY_PAGES_MAX, bytes: limits::HISTORY_BYTES_MAX, records: limits::HISTORY_RECORDS_MAX, work: limits::HISTORY_WORK_MAX }
    }
}
impl Budget {
    pub fn request(&mut self) -> Result<usize, Unavailable> {
        self.pages = self.pages.checked_sub(1).ok_or(Unavailable::Budget)?;
        if self.bytes == 0 { return Err(Unavailable::Budget) }
        Ok(self.bytes.min(limits::ANSWER_MAX_BYTES))
    }
    pub fn bytes(&mut self, bytes: usize) -> Result<(), Unavailable> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(Unavailable::Budget)?;
        Ok(())
    }
    /// Metadata routes already have the API's 2 MiB body cap; reserve that
    /// cap before the call, so it also belongs to this lookup's byte budget.
    pub fn metadata(&mut self) -> Result<(), Unavailable> {
        self.request()?;
        self.bytes(limits::ANSWER_MAX_BYTES)
    }
    pub fn scan(&mut self, records: usize) -> Result<(), Unavailable> {
        let cost = records.saturating_mul(records).saturating_add(records.saturating_mul(16));
        self.work = self.work.checked_sub(cost).ok_or(Unavailable::Budget)?;
        Ok(())
    }
    pub fn page(&mut self, records: &[Record], channel: &str, after: u64, upto: u64) -> Result<(), Unavailable> {
        if records.len() > limits::NOTE_PAGE_RECORDS as usize { return Err(Unavailable::Corrupt) }
        self.records = self.records.checked_sub(records.len()).ok_or(Unavailable::Budget)?;
        let mut expected = after.checked_add(1).ok_or(Unavailable::Corrupt)?;
        for r in records {
            if r.channel != channel || r.seq != expected || r.at <= 0 || r.principal.is_empty() || r.body.is_null() { return Err(Unavailable::Gap) }
            expected = expected.checked_add(1).ok_or(Unavailable::Corrupt)?;
        }
        // Newer records may follow the captured window in the same page.
        if expected <= upto { return Err(Unavailable::Gap) }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn rec(seq: u64) -> Record { Record {channel:"work".into(), seq, at:1, principal:"npub1agent".into(),kind:"message".into(),body:json!({"kind":"turn.end"})} }
    #[test]
    fn gaps_short_pages_duplicates_and_wrong_channels_are_unavailable() {
        for records in [vec![],vec![rec(1)],vec![rec(1),rec(3)],vec![rec(1),rec(1)],vec![Record {channel:"chat".into(),..rec(1)},rec(2)]] {
            assert_eq!(Budget::default().page(&records,"work",0,2),Err(Unavailable::Gap));
        }
        assert!(Budget::default().page(&[rec(1),rec(2),rec(3)],"work",0,2).is_ok());
    }
    #[test]
    fn each_budget_is_finite_and_exhaustion_is_explicit() {
        let mut b = Budget::default();
        for _ in 0..limits::HISTORY_PAGES_MAX { b.request().unwrap(); }
        assert_eq!(b.request(),Err(Unavailable::Budget));
        let mut b = Budget::default(); b.bytes(limits::HISTORY_BYTES_MAX).unwrap(); assert_eq!(b.request(),Err(Unavailable::Budget));
        let mut b = Budget::default(); assert_eq!(b.scan(limits::HISTORY_WORK_MAX),Err(Unavailable::Budget));
        let mut b = Budget {records:0,..Budget::default()}; assert_eq!(b.page(&[rec(1)],"work",0,1),Err(Unavailable::Budget));
        assert!(Unavailable::Budget.message().starts_with("History unavailable:"));
    }
}
