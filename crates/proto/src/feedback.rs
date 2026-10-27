//! Feedback's public wire types (docs/api.md, Feedback): what a person
//! sends the deployment's operators from the shell (a problem reported, or
//! a message) and what the operators' admin reads of it. The rules are
//! `fragment_core::feedback`; the registry keeps the rows.

use serde::{Deserialize, Serialize};

/// What a person sends: a problem they hit, or anything else they want
/// the operators to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    Problem,
    Message,
}

impl FeedbackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FeedbackKind::Problem => "problem",
            FeedbackKind::Message => "message",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "problem" => Some(FeedbackKind::Problem),
            "message" => Some(FeedbackKind::Message),
            _ => None,
        }
    }
}

/// One screenshot a feedback holds, by its place in it (from 0): its
/// type, read from its bytes, and its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screenshot {
    pub mime: String,
    pub size: u64,
}

/// One feedback, as the operators read it. `email` is its sender's
/// latest sign-in's when they sent it; `page`, the shell's path they sent
/// it from; `userAgent`, their browser's; `release`, the platform's
/// release that took it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    pub id: String,
    pub n: u64,
    pub person: String,
    pub email: Option<String>,
    pub kind: FeedbackKind,
    pub message: String,
    pub page: Option<String>,
    pub user_agent: Option<String>,
    pub release: String,
    pub screenshots: Vec<Screenshot>,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
}

/// `POST /api/feedback`'s answer: what was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSent {
    pub id: String,
    pub screenshots: u64,
}

/// A page of feedback, newest first; `next`, the `before` of the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackPage {
    pub feedback: Vec<Feedback>,
    pub next: Option<u64>,
}

/// `PATCH /api/admin/feedback/{id}`'s body: an operator marks it done
/// with, or open again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetResolved {
    pub resolved: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each kind round-trips by its name; anything else is none.
    #[test]
    fn a_kind_is_a_problem_or_a_message() {
        for k in [FeedbackKind::Problem, FeedbackKind::Message] {
            assert_eq!(FeedbackKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
        assert_eq!(FeedbackKind::parse("bug"), None);
        assert_eq!(FeedbackKind::parse("Problem"), None);
    }
}
