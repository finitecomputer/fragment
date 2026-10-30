//! An org's content governance (docs/cloudflare-v1.md, decisions 69 to
//! 71): who of its people are in its content, at which epoch, and which
//! of them are its content admins. A role apart from its billing: a seat
//! or a billing admin's place gives none of it, and a seatless person may
//! hold it. The registry keeps the rows (`org_content`); these are the
//! rules it applies, each a pure function of what it read.
//!
//! - **Appointed, never inherited** (decision 69). An admin's place is an
//!   offer to an email, no one's until whoever signs in with it accepts
//!   it. An org's content admins offer it. Who pays for it offers the
//!   first, while its content has had no one at all; an org left with no
//!   admin after that (a sole admin's wipe) waits on an operator's offer,
//!   logged. Billing admins are never promoted to it.
//! - **The last admin stays.** Neither stepping down nor offboarding
//!   leaves an org with no content admin.
//! - **Epochs fence.** Offboarding a person raises their epoch and makes
//!   them inactive at once: whatever was granted them under the old one
//!   opens nothing, before anything is cleaned up. Coming back is a new
//!   yes, at the raised epoch, so an old grant never wakes.
//! - **One answer.** An offer is answered once: the same answer again
//!   answers as the first did, the other is refused.

use fragment_proto::ErrorCode;
use serde::{Deserialize, Serialize};

/// Content admins an org holds at most, offers waiting included.
pub const ADMINS_MAX: u64 = 32;
/// Entries of an org's governance log a page holds.
pub const LOG_PAGE: u64 = 100;
/// A request id, as a client names its change (a replay finds the first).
pub const REQUEST_MAX_BYTES: usize = 128;

/// A person's place in an org's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// From 1; raised by each offboarding.
    pub epoch: u64,
    pub active: bool,
    pub admin: bool,
}

impl Place {
    fn assert_valid(self) {
        assert!(self.epoch >= 1, "an epoch counts from 1");
        assert!(self.active || !self.admin, "an admin is active");
    }
}

/// Who offers a content admin's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offerer {
    /// One of the org's active content admins.
    Admin,
    /// Who pays for the org: its first admin's offer alone.
    Payer,
    /// The deployment's operator.
    Operator,
    /// Anyone else: a billing admin, a seat's holder, a stranger.
    Other,
}

/// An offer's answer, as its person gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Accepted,
    Declined,
}

impl Answer {
    pub fn as_str(self) -> &'static str {
        match self {
            Answer::Accepted => "accepted",
            Answer::Declined => "declined",
        }
    }

    pub fn parse(s: &str) -> Option<Answer> {
        match s {
            "accepted" => Some(Answer::Accepted),
            "declined" => Some(Answer::Declined),
            _ => None,
        }
    }
}

/// Why a governance change is refused. A refusal changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Only an org's content admins do that (and, before it has one, who
    /// pays for it).
    NotAdmin,
    /// The org has, or has had, content admins: one of them offers the
    /// next, or, with none left, an operator.
    HasAdmins,
    /// The org holds `ADMINS_MAX` admins, offers waiting included.
    AdminsFull,
    /// The offer was answered the other way.
    Answered(Answer),
    /// It would leave the org with no content admin.
    LastAdmin,
    /// The person's epoch is not the one the request expected.
    StaleEpoch { current: u64 },
    /// The person is not in the org's content now.
    NotIn,
}

impl Refusal {
    pub fn code(self) -> ErrorCode {
        match self {
            Refusal::NotAdmin | Refusal::HasAdmins => ErrorCode::Forbidden,
            Refusal::AdminsFull => ErrorCode::InvalidRequest,
            Refusal::Answered(_) | Refusal::LastAdmin | Refusal::StaleEpoch { .. } => ErrorCode::ConflictingBody,
            Refusal::NotIn => ErrorCode::NotFound,
        }
    }

    pub fn message(self) -> String {
        match self {
            Refusal::NotAdmin => "only the org's content admins do this; a billing admin's place gives none of it".into(),
            Refusal::HasAdmins => "the org's content has its admins: one of them offers the next (with none left, an operator does)".into(),
            Refusal::AdminsFull => format!("an org has at most {ADMINS_MAX} content admins, offers waiting included"),
            Refusal::Answered(a) => format!("this offer was {} already", a.as_str()),
            Refusal::LastAdmin => "the org's last content admin stays: another accepts the place first".into(),
            Refusal::StaleEpoch { current } => format!("their epoch is {current} now: read the org's content again"),
            Refusal::NotIn => "they are not in the org's content".into(),
        }
    }
}

/// Whether `offerer` may offer a content admin's place, the org's content
/// holding `people` (anyone it ever took in), `admins` (active ones) and
/// `waiting` offers.
pub fn may_offer(offerer: Offerer, people: u64, admins: u64, waiting: u64) -> Result<(), Refusal> {
    assert!(admins <= people, "an admin is one of its people");
    match offerer {
        Offerer::Admin => {}
        Offerer::Payer if people == 0 => {}
        Offerer::Operator if admins == 0 => {}
        Offerer::Payer | Offerer::Operator => return Err(Refusal::HasAdmins),
        Offerer::Other => return Err(Refusal::NotAdmin),
    }
    if admins + waiting >= ADMINS_MAX {
        return Err(Refusal::AdminsFull);
    }
    Ok(())
}

/// An answer to an offer `recorded` answered (or not): `Ok(true)` to apply
/// it, `Ok(false)` a replay of the same answer, or refused.
pub fn answer(recorded: Option<Answer>, asked: Answer) -> Result<bool, Refusal> {
    match recorded {
        None => Ok(true),
        Some(a) if a == asked => Ok(false),
        Some(a) => Err(Refusal::Answered(a)),
    }
}

/// A person's place once they accept an admin's: in at once, at their
/// epoch (one an offboarding raised stays raised).
pub fn accepted(place: Option<Place>) -> Place {
    let next = match place {
        None => Place { epoch: 1, active: true, admin: true },
        Some(p) => Place { active: true, admin: true, ..p },
    };
    next.assert_valid();
    next
}

/// A content admin stepping down or removed: an admin no more, still in
/// the org's content. The org's last admin stays.
pub fn removed_admin(place: Place, admins: u64) -> Result<Place, Refusal> {
    place.assert_valid();
    if !place.admin {
        return Ok(place);
    }
    if admins <= 1 {
        return Err(Refusal::LastAdmin);
    }
    Ok(Place { admin: false, ..place })
}

/// A person offboarded at `expected` epoch: out at once, at the next.
/// Never the org's last admin.
pub fn offboarded(place: Place, expected: u64, admins: u64) -> Result<Place, Refusal> {
    place.assert_valid();
    if place.epoch != expected {
        return Err(Refusal::StaleEpoch { current: place.epoch });
    }
    if !place.active {
        return Err(Refusal::NotIn);
    }
    if place.admin && admins <= 1 {
        return Err(Refusal::LastAdmin);
    }
    let next = Place { epoch: place.epoch + 1, active: false, admin: false };
    next.assert_valid();
    Ok(next)
}

/// A request id: 1 to `REQUEST_MAX_BYTES` printable ASCII, no spaces.
pub fn valid_request(id: &str) -> bool {
    crate::price::printable(id, REQUEST_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IN: Place = Place { epoch: 1, active: true, admin: false };
    const ADMIN: Place = Place { epoch: 1, active: true, admin: true };

    /// Valid: an accepted admin offers, steps others down, offboards.
    #[test]
    fn accepted_admin_controls() {
        assert_eq!(accepted(None), ADMIN);
        assert_eq!(may_offer(Offerer::Admin, 1, 1, 0), Ok(()));
        assert_eq!(removed_admin(ADMIN, 2), Ok(IN));
        assert_eq!(offboarded(ADMIN, 1, 2), Ok(Place { epoch: 2, active: false, admin: false }));
        assert_eq!(offboarded(IN, 1, 1), Ok(Place { epoch: 2, active: false, admin: false }));
        assert_eq!(removed_admin(IN, 1), Ok(IN), "no admin to remove: nothing changes");
    }

    /// Invalid: a billing admin, a seat, a stranger hold no content power;
    /// who pays and an operator only name the first admin; the last stays.
    #[test]
    fn billing_admin_has_no_content_power() {
        assert_eq!(may_offer(Offerer::Other, 0, 0, 0), Err(Refusal::NotAdmin));
        assert_eq!(may_offer(Offerer::Other, 3, 3, 0), Err(Refusal::NotAdmin));
        assert_eq!(may_offer(Offerer::Payer, 0, 0, 0), Ok(()), "the first");
        assert_eq!(may_offer(Offerer::Payer, 1, 1, 0), Err(Refusal::HasAdmins));
        assert_eq!(may_offer(Offerer::Payer, 2, 0, 0), Err(Refusal::HasAdmins), "a sole admin wiped: an operator's to recover, not who pays");
        assert_eq!(may_offer(Offerer::Operator, 2, 0, 2), Ok(()));
        assert_eq!(may_offer(Offerer::Operator, 1, 1, 0), Err(Refusal::HasAdmins));
        assert_eq!(may_offer(Offerer::Admin, ADMINS_MAX - 1, ADMINS_MAX - 1, 1), Err(Refusal::AdminsFull));
        assert_eq!(removed_admin(ADMIN, 1), Err(Refusal::LastAdmin));
        assert_eq!(offboarded(ADMIN, 1, 1), Err(Refusal::LastAdmin));
        assert_eq!(offboarded(IN, 2, 1), Err(Refusal::StaleEpoch { current: 1 }));
        let out = Place { epoch: 2, active: false, admin: false };
        assert_eq!(offboarded(out, 2, 1), Err(Refusal::NotIn));
        for r in [Refusal::NotAdmin, Refusal::HasAdmins, Refusal::LastAdmin, Refusal::NotIn] {
            assert!(!r.message().is_empty() && r.code().status() >= 400);
        }
    }

    /// Replay: one answer, again as the first; the other is a conflict.
    #[test]
    fn role_answer_conflict() {
        assert_eq!(answer(None, Answer::Accepted), Ok(true));
        assert_eq!(answer(Some(Answer::Accepted), Answer::Accepted), Ok(false));
        assert_eq!(answer(Some(Answer::Accepted), Answer::Declined), Err(Refusal::Answered(Answer::Accepted)));
        assert_eq!(answer(Some(Answer::Declined), Answer::Accepted), Err(Refusal::Answered(Answer::Declined)));
        assert_eq!(Refusal::Answered(Answer::Declined).code().status(), 409);
        for a in [Answer::Accepted, Answer::Declined] {
            assert_eq!(Answer::parse(a.as_str()), Some(a));
        }
    }

    /// Restart: the fence is the stored place; read back, it holds, and a
    /// new yes comes back at the raised epoch, never the old one.
    #[test]
    fn offboard_fence_survives_restart() {
        let out = offboarded(ADMIN, 1, 2).unwrap();
        let kept: Place = serde_json::from_str(&serde_json::to_string(&out).unwrap()).unwrap();
        assert_eq!(kept, out);
        assert!(!kept.active && kept.epoch == 2, "a grant bound to epoch 1 opens nothing");
        let back = accepted(Some(kept));
        assert_eq!(back, Place { epoch: 2, active: true, admin: true });
        assert_ne!(back.epoch, ADMIN.epoch, "coming back never wakes an old grant");
    }
}
