//! An org's content governance (docs/cloudflare-v1.md, decisions 69 to
//! 71): its content admins, a role apart from its billing. A seat or a
//! billing admin's place gives none of it, and a seatless person may hold
//! it. The registry keeps the rows (`org_content`, `org_content_offers`);
//! these are the rules it applies, each a pure function of what it read.
//!
//! - **Appointed, never automatically** (decision 69). An admin's place is
//!   an offer to an email, no one's until whoever signs in with it
//!   accepts it. The offer records its grounds, which must still hold when
//!   it is accepted: an admin's, while they are one; who pays for the
//!   org's, its first, while its content has had no one (they may name
//!   themselves); an operator's, the recovery of an org with no admin (a
//!   sole admin's wipe). No billing admin is promoted by being one.
//! - **The last admin stays.** Stepping down never leaves an org with no
//!   content admin.
//! - **Offers end.** Each is answered once (the same answer again answers
//!   as the first did; the other is refused), withdrawn by the org's
//!   admins, or expires after `OFFER_TTL_MS`. A person stepping down takes
//!   their waiting offers with them, made or received.
//! - **Epochs fence.** A person is in the org's content at an epoch, from
//!   their yes to an admin's place or an invite to one of its fragments.
//!   Offboarding raises it and makes them inactive at once: whatever its
//!   fragments granted them under the old one opens nothing, before
//!   anything is cleaned up there. Coming back is a new yes, at the raised
//!   epoch; an offer or an invite made before they left admits them no
//!   more (`left_at`).

use fragment_proto::ErrorCode;
use serde::{Deserialize, Serialize};

/// Content admins an org holds at most, its admins' offers waiting
/// included (an operator's recovery is never refused for it).
pub const ADMINS_MAX: u64 = 32;
/// How long an offer waits for its answer.
pub const OFFER_TTL_MS: i64 = 7 * 24 * 3_600_000;
/// Orgs' content a person is in at once: every request of theirs carries
/// their places (`Identity::orgs`).
pub const ORGS_PER_PERSON_MAX: u64 = 64;
/// Entries of an org's governance log a page holds.
pub const LOG_PAGE: u64 = 100;
/// A request id, as a client names its change (a replay finds the first).
pub const REQUEST_MAX_BYTES: usize = 128;

/// Who offers a content admin's place, and on what grounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grounds {
    /// One of the org's content admins.
    Admin,
    /// Who pays for the org: its first admin alone.
    Payer,
    /// The deployment's operator: the recovery of an org with no admin.
    Operator,
}

impl Grounds {
    pub fn as_str(self) -> &'static str {
        match self {
            Grounds::Admin => "admin",
            Grounds::Payer => "payer",
            Grounds::Operator => "operator",
        }
    }

    pub fn parse(s: &str) -> Option<Grounds> {
        match s {
            "admin" => Some(Grounds::Admin),
            "payer" => Some(Grounds::Payer),
            "operator" => Some(Grounds::Operator),
            _ => None,
        }
    }
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
}

/// Where an offer stands, as its row says at `now`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Waiting,
    Answered(Answer),
    Withdrawn,
    Expired,
}

impl Standing {
    /// From its row: its answer column and when it expires.
    pub fn of(answer: Option<&str>, expires_at: i64, now: i64) -> Option<Standing> {
        Some(match answer {
            None if now >= expires_at => Standing::Expired,
            None => Standing::Waiting,
            Some("accepted") => Standing::Answered(Answer::Accepted),
            Some("declined") => Standing::Answered(Answer::Declined),
            Some("withdrawn") => Standing::Withdrawn,
            Some("expired") => Standing::Expired,
            Some(_) => return None,
        })
    }
}

/// What a person's answer finds the org as, for its grounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// Whoever its content has ever taken in.
    pub people: u64,
    /// Its content admins now.
    pub admins: u64,
    /// Whether who made the offer still stands as its grounds say: an
    /// admin of the org's, or who pays for it.
    pub offerer_stands: bool,
}

/// Why a governance change is refused. A refusal changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Only an org's content admins do that (and, for its first, who pays
    /// for it).
    NotAdmin,
    /// The org's content has had its admins: one of them offers the next
    /// (with none left, an operator does).
    HasAdmins,
    /// The org holds `ADMINS_MAX` admins, its admins' offers included.
    AdminsFull,
    /// The offer was answered the other way.
    Answered(Answer),
    /// The offer was withdrawn, or expired, unanswered.
    Ended,
    /// What the offer was made on holds no more.
    Lapsed(Grounds),
    /// It would leave the org with no content admin.
    LastAdmin,
    /// The person is not one of the org's content admins.
    NotIn,
    /// The person's epoch is not the one the request expected.
    StaleEpoch { current: u64 },
    /// Made before the person last left the org's content: it admits them
    /// no more.
    LeftSince,
}

impl Refusal {
    pub fn code(self) -> ErrorCode {
        match self {
            Refusal::NotAdmin | Refusal::HasAdmins => ErrorCode::Forbidden,
            Refusal::AdminsFull => ErrorCode::InvalidRequest,
            Refusal::Answered(_) | Refusal::Lapsed(_) | Refusal::LastAdmin | Refusal::StaleEpoch { .. } => ErrorCode::ConflictingBody,
            Refusal::Ended | Refusal::NotIn => ErrorCode::NotFound,
            Refusal::LeftSince => ErrorCode::Forbidden,
        }
    }

    pub fn message(self) -> String {
        match self {
            Refusal::NotAdmin => "only the org's content admins do this; a billing admin's place gives none of it".into(),
            Refusal::HasAdmins => "the org's content has had its admins: one of them offers the next (with none left, an operator does)".into(),
            Refusal::AdminsFull => format!("an org has at most {ADMINS_MAX} content admins, its admins' offers waiting included: withdraw one first"),
            Refusal::Answered(a) => format!("this offer was {} already", a.as_str()),
            Refusal::Ended => "this offer was withdrawn, or expired, unanswered".into(),
            Refusal::Lapsed(Grounds::Admin) => "whoever made this offer is no longer the org's content admin: ask for a new one".into(),
            Refusal::Lapsed(Grounds::Payer) => "this offer was the org's first admin's, and the org has had one since: ask its admins".into(),
            Refusal::Lapsed(Grounds::Operator) => "this offer was the recovery of an org with no admin, and it has one now: ask its admins".into(),
            Refusal::LastAdmin => "the org's last content admin stays: another accepts the place first".into(),
            Refusal::NotIn => "they are not in the org's content".into(),
            Refusal::StaleEpoch { current } => format!("their epoch is {current} now: read the org's content again"),
            Refusal::LeftSince => "this was made before they left the org's content: only a new one admits them".into(),
        }
    }
}

/// Whether `offerer` may offer a content admin's place, the org's content
/// having taken in `people` ever, holding `admins` now and its admins'
/// unexpired offers `waiting`.
pub fn may_offer(offerer: Option<Grounds>, people: u64, admins: u64, waiting: u64) -> Result<Grounds, Refusal> {
    assert!(admins <= people, "an admin is one of its people");
    let grounds = match offerer {
        Some(Grounds::Admin) if admins + waiting >= ADMINS_MAX => return Err(Refusal::AdminsFull),
        Some(Grounds::Payer) if people > 0 => return Err(Refusal::HasAdmins),
        Some(Grounds::Payer) if waiting >= ADMINS_MAX => return Err(Refusal::AdminsFull),
        Some(Grounds::Operator) if admins > 0 => return Err(Refusal::HasAdmins),
        Some(g) => g,
        None => return Err(Refusal::NotAdmin),
    };
    Ok(grounds)
}

/// An answer to an offer standing as `standing`: `Ok(true)` to apply it,
/// `Ok(false)` a replay of the same answer, or refused. An accept also
/// needs its grounds to hold now (`grounds_hold`).
pub fn answer(standing: Standing, asked: Answer) -> Result<bool, Refusal> {
    match standing {
        Standing::Waiting => Ok(true),
        Standing::Answered(a) if a == asked => Ok(false),
        Standing::Answered(a) => Err(Refusal::Answered(a)),
        Standing::Withdrawn | Standing::Expired => Err(Refusal::Ended),
    }
}

/// Whether an offer made on `grounds` may be accepted now.
pub fn grounds_hold(grounds: Grounds, now: Now) -> Result<(), Refusal> {
    let holds = match grounds {
        Grounds::Admin => now.offerer_stands,
        Grounds::Payer => now.offerer_stands && now.people == 0,
        Grounds::Operator => now.admins == 0,
    };
    match holds {
        true => Ok(()),
        false => Err(Refusal::Lapsed(grounds)),
    }
}

/// An admin stepping down, or stepped down, the org holding `admins`:
/// whether anything changes (none for one who is no admin). Never its last.
pub fn steps_down(admin: bool, admins: u64) -> Result<bool, Refusal> {
    match (admin, admins) {
        (false, _) => Ok(false),
        (true, 0..=1) => Err(Refusal::LastAdmin),
        (true, _) => Ok(true),
    }
}

/// A person's place in an org's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A person's place once they accept an admin's offer made at
/// `offered_at`: in, an admin, at their epoch (one an offboarding raised
/// stays raised), unless they left since it was made.
pub fn accepted(place: Option<Place>, left_at: Option<i64>, offered_at: i64) -> Result<Place, Refusal> {
    if left_at.is_some_and(|left| left > offered_at) {
        return Err(Refusal::LeftSince);
    }
    let next = match place {
        None => Place { epoch: 1, active: true, admin: true },
        Some(p) => Place { active: true, admin: true, ..p },
    };
    next.assert_valid();
    Ok(next)
}

/// A person coming into the org's content as they come into one of its
/// fragments, by their yes to an invite made at `invited_at` (none: a
/// content admin's own add): their place, and whether it is new to them.
pub fn enrolled(place: Option<Place>, left_at: Option<i64>, invited_at: Option<i64>) -> Result<(Place, bool), Refusal> {
    match place {
        None => Ok((Place { epoch: 1, active: true, admin: false }, true)),
        Some(p) if p.active => Ok((p, false)),
        Some(_) if invited_at.zip(left_at).is_some_and(|(invited, left)| invited < left) => Err(Refusal::LeftSince),
        Some(p) => Ok((Place { active: true, admin: false, ..p }, true)),
    }
}

/// A person offboarded at `expected` epoch, the org holding `admins`: out
/// at once, at the next, an admin no more. Never its last admin.
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

/// A client's request id: 1 to `REQUEST_MAX_BYTES` printable ASCII, no
/// spaces. It is kept prefixed by its kind (`offer:`), apart from the
/// platform's own.
pub fn valid_request(id: &str) -> bool {
    crate::price::printable(id, REQUEST_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_790_812_800_000;
    const NONE_YET: Now = Now { people: 0, admins: 0, offerer_stands: true };

    /// Valid: an accepted admin offers, withdraws, steps others down.
    #[test]
    fn accepted_admin_controls() {
        assert_eq!(may_offer(Some(Grounds::Admin), 1, 1, 0), Ok(Grounds::Admin));
        assert_eq!(grounds_hold(Grounds::Admin, Now { people: 1, admins: 1, offerer_stands: true }), Ok(()));
        assert_eq!(steps_down(true, 2), Ok(true));
        assert_eq!(steps_down(false, 1), Ok(false), "no admin to step down: nothing changes");
        assert_eq!(Standing::of(Some("withdrawn"), T + OFFER_TTL_MS, T), Some(Standing::Withdrawn));
    }

    /// Invalid: a billing admin, a seat, a stranger hold no content power;
    /// who pays names only the first, an operator recovers only an org
    /// with no admin; the last admin stays; offers lapse with their grounds.
    #[test]
    fn billing_admin_has_no_content_power() {
        assert_eq!(may_offer(None, 0, 0, 0), Err(Refusal::NotAdmin));
        assert_eq!(may_offer(None, 3, 3, 0), Err(Refusal::NotAdmin));
        assert_eq!(may_offer(Some(Grounds::Payer), 0, 0, 0), Ok(Grounds::Payer), "the first");
        assert_eq!(may_offer(Some(Grounds::Payer), 1, 1, 0), Err(Refusal::HasAdmins));
        assert_eq!(may_offer(Some(Grounds::Payer), 2, 0, 0), Err(Refusal::HasAdmins), "a sole admin wiped: an operator's to recover, not who pays");
        assert_eq!(may_offer(Some(Grounds::Operator), 2, 0, ADMINS_MAX), Ok(Grounds::Operator), "a recovery is never capped by waiting offers");
        assert_eq!(may_offer(Some(Grounds::Operator), 1, 1, 0), Err(Refusal::HasAdmins));
        assert_eq!(may_offer(Some(Grounds::Admin), ADMINS_MAX - 1, ADMINS_MAX - 1, 1), Err(Refusal::AdminsFull));
        assert_eq!(steps_down(true, 1), Err(Refusal::LastAdmin));
        // who pays offered many "firsts": once one is accepted, the rest lapse
        assert_eq!(grounds_hold(Grounds::Payer, NONE_YET), Ok(()));
        assert_eq!(grounds_hold(Grounds::Payer, Now { people: 1, admins: 1, offerer_stands: true }), Err(Refusal::Lapsed(Grounds::Payer)));
        assert_eq!(grounds_hold(Grounds::Payer, Now { offerer_stands: false, ..NONE_YET }), Err(Refusal::Lapsed(Grounds::Payer)), "who paid no more");
        // an admin who made it and stepped down: it lapses with them
        assert_eq!(grounds_hold(Grounds::Admin, Now { people: 2, admins: 1, offerer_stands: false }), Err(Refusal::Lapsed(Grounds::Admin)));
        assert_eq!(grounds_hold(Grounds::Operator, Now { people: 2, admins: 1, offerer_stands: true }), Err(Refusal::Lapsed(Grounds::Operator)));
        for r in [Refusal::NotAdmin, Refusal::HasAdmins, Refusal::LastAdmin, Refusal::NotIn, Refusal::Ended, Refusal::Lapsed(Grounds::Admin)] {
            assert!(!r.message().is_empty() && r.code().status() >= 400);
        }
    }

    /// Replay: one answer, again as the first; the other is a conflict; an
    /// offer withdrawn or expired takes none.
    #[test]
    fn role_answer_conflict() {
        assert_eq!(answer(Standing::Waiting, Answer::Accepted), Ok(true));
        assert_eq!(answer(Standing::Answered(Answer::Accepted), Answer::Accepted), Ok(false));
        assert_eq!(answer(Standing::Answered(Answer::Accepted), Answer::Declined), Err(Refusal::Answered(Answer::Accepted)));
        assert_eq!(answer(Standing::Answered(Answer::Declined), Answer::Accepted), Err(Refusal::Answered(Answer::Declined)));
        assert_eq!(answer(Standing::Withdrawn, Answer::Accepted), Err(Refusal::Ended));
        assert_eq!(answer(Standing::Expired, Answer::Declined), Err(Refusal::Ended));
        assert_eq!(Refusal::Answered(Answer::Declined).code().status(), 409);
        for g in [Grounds::Admin, Grounds::Payer, Grounds::Operator] {
            assert_eq!(Grounds::parse(g.as_str()), Some(g));
        }
    }

    /// Offboarding: out at once at the next epoch, never the last admin,
    /// at the epoch read; coming back is at the raised one, and what was
    /// made before they left admits them no more.
    #[test]
    fn offboarding_raises_the_epoch_and_fences_what_came_before() {
        const ADMIN: Place = Place { epoch: 1, active: true, admin: true };
        const IN: Place = Place { epoch: 1, active: true, admin: false };
        let out = offboarded(IN, 1, 1).unwrap();
        assert_eq!(out, Place { epoch: 2, active: false, admin: false });
        assert_eq!(offboarded(ADMIN, 1, 2), Ok(Place { epoch: 2, active: false, admin: false }));
        assert_eq!(offboarded(ADMIN, 1, 1), Err(Refusal::LastAdmin));
        assert_eq!(offboarded(IN, 2, 1), Err(Refusal::StaleEpoch { current: 1 }));
        assert_eq!(offboarded(out, 2, 1), Err(Refusal::NotIn));
        // an invite, or an offer, made before they left admits no one
        let left = T + 10;
        assert_eq!(enrolled(Some(out), Some(left), Some(T)), Err(Refusal::LeftSince));
        assert_eq!(accepted(Some(out), Some(left), T), Err(Refusal::LeftSince));
        // a new one admits them at the raised epoch, never the old
        assert_eq!(enrolled(Some(out), Some(left), Some(left + 1)), Ok((Place { epoch: 2, active: true, admin: false }, true)));
        assert_eq!(accepted(Some(out), Some(left), left + 1), Ok(Place { epoch: 2, active: true, admin: true }));
        assert_eq!(enrolled(None, None, Some(T)), Ok((IN, true)));
        assert_eq!(enrolled(Some(IN), None, None), Ok((IN, false)), "in already: their epoch, nothing new");
    }

    /// Restart: an offer's standing is its stored row read at a later
    /// time: waiting until it expires, and expired after, unanswered.
    #[test]
    fn an_offer_expires_across_a_restart() {
        let expires = T + OFFER_TTL_MS;
        assert_eq!(Standing::of(None, expires, T), Some(Standing::Waiting));
        assert_eq!(Standing::of(None, expires, expires - 1), Some(Standing::Waiting));
        assert_eq!(Standing::of(None, expires, expires), Some(Standing::Expired));
        assert_eq!(Standing::of(Some("accepted"), expires, expires + 1), Some(Standing::Answered(Answer::Accepted)), "an answer outlives its expiry");
        assert_eq!(Standing::of(Some("maybe"), expires, T), None);
    }
}
