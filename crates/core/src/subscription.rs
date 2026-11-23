//! A channel subscription's rules (cell/src/subscriptions.rs): whether a
//! record goes to its subscriber, and what the delivery queue's consumer
//! does with one it holds.
//!
//! A subscription is made by a member who may read its channel, but
//! access narrows after that: a member is lowered, an agent beside its
//! person is held below it, a deploy raises the channel's read role or
//! drops the channel. So each delivery asks again, of the subscriber's
//! standing and the channel's read role as they are when it is sent, and
//! a record goes only to someone who may read it now. The subscriber is
//! judged as itself, by `permit::decide`, as a signed request of its own
//! would be: its membership, capped by its hold where one applies (an
//! agent beside its person, rule 4). The subscription is kept: should the
//! access come back, it delivers again.
//!
//! A record delivery waits on the queue after the fragment's outbox
//! queued it, and is retried from there. So the consumer asks the
//! fragment again before each send (`at_send`): the queue holds the URL
//! and the record, and only the fragment knows whether its subscriber
//! still reads them.

use fragment_proto::{Role, Visibility};

use crate::permit::{decide, Action, Facts, Standing, Verdict};

/// Whether a record goes to a subscriber with `standing` (as itself) in a
/// fragment of `visibility`, on a channel whose read role is now `read`
/// (`None`: no longer declared, so read by no one). Only a member is
/// delivered to: a subscription ends with its membership, and what
/// anyone may read of a public fragment is not a reason to keep sending.
pub fn delivers(visibility: Visibility, standing: Standing, read: Option<Role>) -> bool {
    let Some(read) = read else { return false };
    standing.member.is_some() && matches!(decide(&Facts { visibility, standing, action: Action::Read(read) }), Verdict::Allow(_))
}

/// What the fragment answered when the consumer asked whether a queued
/// record may still go (`delivers`, asked by the fragment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Yes,
    No,
    /// No answer came (unreachable, another release, an error).
    Unanswered,
}

/// What the consumer does with one queued record delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtSend {
    Send,
    /// Acked unsent: its subscriber may not read it now.
    Withhold,
    /// Tried again later: whether it may go is not known.
    Retry,
}

/// The consumer's step for a queued record, from what its fragment said:
/// `None` when it could not be asked (a delivery that names no
/// subscription), which never sends.
pub fn at_send(asked: Option<Asked>) -> AtSend {
    match asked {
        Some(Asked::Yes) => AtSend::Send,
        Some(Asked::No) | None => AtSend::Withhold,
        Some(Asked::Unanswered) => AtSend::Retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permit::AGENT_ROLE_MAX;

    const ROLES: [Role; 4] = [Role::Public, Role::Viewer, Role::Editor, Role::Owner];
    const VISIBILITIES: [Visibility; 3] = [Visibility::Members, Visibility::Link, Visibility::Public];

    fn member(role: Role) -> Standing {
        Standing { member: Some(role), signed: true, ..Standing::default() }
    }

    /// An agent beside its person (decision 64), held at `held`.
    fn held_beside(role: Role, held: Role) -> Standing {
        Standing { held: Some(held), beside_person: true, ..member(role) }
    }

    /// Valid: a member whose role still reads the channel is delivered to.
    #[test]
    fn a_member_who_still_reads_is_delivered_to() {
        for v in VISIBILITIES {
            assert!(delivers(v, member(Role::Viewer), Some(Role::Viewer)));
            assert!(delivers(v, member(Role::Editor), Some(Role::Viewer)));
            assert!(delivers(v, member(Role::Owner), Some(Role::Owner)));
        }
    }

    /// Invalid: lowered below the channel's read role (a member downgrade),
    /// or the channel's read role raised past theirs (a deploy), or no
    /// member, or the channel gone: nothing goes.
    #[test]
    fn narrowed_access_stops_delivery() {
        let v = Visibility::Members;
        assert!(!delivers(v, member(Role::Viewer), Some(Role::Editor)), "an editor lowered to viewer on an editors' channel");
        assert!(!delivers(v, member(Role::Editor), Some(Role::Owner)), "a channel raised to owner");
        assert!(!delivers(v, Standing::default(), Some(Role::Viewer)), "no longer a member");
        for role in ROLES {
            assert!(!delivers(v, member(role), None), "{role:?} on a channel no longer declared");
        }
    }

    /// Invalid: no longer a member of a public fragment, whose floor reads
    /// a public channel, is still sent nothing.
    #[test]
    fn a_public_floor_is_not_a_subscription() {
        let signed = Standing { signed: true, ..Standing::default() };
        assert!(!delivers(Visibility::Public, signed, Some(Role::Public)));
        assert!(!delivers(Visibility::Link, Standing { link: true, ..signed }, Some(Role::Viewer)));
    }

    /// Invalid: an agent beside its person keeps its stored editor role,
    /// but its hold at viewer is what it reads with: an editors' channel
    /// goes to it no more, a viewers' channel still does.
    #[test]
    fn a_held_agent_beside_its_person_reads_with_its_hold() {
        for v in VISIBILITIES {
            assert!(!delivers(v, held_beside(Role::Editor, Role::Viewer), Some(Role::Editor)), "{v:?}");
            assert!(delivers(v, held_beside(Role::Editor, Role::Viewer), Some(Role::Viewer)), "{v:?}");
        }
        // a hold limits an agent beside its person, never its own
        // memberships elsewhere (permit rule 4)
        let alone = Standing { held: Some(Role::Viewer), ..member(Role::Editor) };
        assert!(delivers(Visibility::Members, alone, Some(Role::Editor)));
    }

    /// Every pair, and every hold beside a person: delivered exactly when
    /// the lower of the role and the hold reads the channel now.
    #[test]
    fn delivered_exactly_when_the_effective_role_reads_now() {
        for role in ROLES {
            for read in ROLES {
                assert_eq!(delivers(Visibility::Members, member(role), Some(read)), role >= read, "{role:?} on a channel {read:?} reads");
                for held in [Role::Viewer, AGENT_ROLE_MAX] {
                    let effective = role.min(held);
                    assert_eq!(delivers(Visibility::Members, held_beside(role, held), Some(read)), effective >= read, "{role:?} held at {held:?} on {read:?}");
                }
            }
        }
    }

    /// The consumer sends only on the fragment's yes; a no, or a delivery
    /// it cannot ask about, is withheld; no answer is tried again.
    #[test]
    fn the_consumer_sends_only_on_a_yes() {
        assert_eq!(at_send(Some(Asked::Yes)), AtSend::Send);
        assert_eq!(at_send(Some(Asked::No)), AtSend::Withhold, "revoked after it was queued");
        assert_eq!(at_send(None), AtSend::Withhold, "names no subscription");
        assert_eq!(at_send(Some(Asked::Unanswered)), AtSend::Retry);
    }
}
