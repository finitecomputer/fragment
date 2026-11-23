//! A channel subscription's one rule (cell/src/subscriptions.rs): whether
//! a record still goes to its subscriber. A subscription is made by a
//! member whose role may read its channel, but roles narrow after that: a
//! member is lowered, or a deploy raises the channel's read role, or
//! drops the channel. So each delivery asks again, of the subscriber's
//! role and the channel's read role as they are when it is sent, and a
//! record goes only to someone who may read it now. The subscription is
//! kept: should the role come back, it delivers again.

use fragment_proto::Role;

/// Whether a record goes to a subscriber whose role in the fragment is now
/// `member` (`None`: no longer a member), on a channel whose read role is
/// now `read` (`None`: no longer declared, so read by no one).
pub fn delivers(member: Option<Role>, read: Option<Role>) -> bool {
    matches!((member, read), (Some(member), Some(read)) if member >= read)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLES: [Role; 4] = [Role::Public, Role::Viewer, Role::Editor, Role::Owner];

    /// Valid: a member whose role still reads the channel is delivered to.
    #[test]
    fn a_member_who_still_reads_is_delivered_to() {
        assert!(delivers(Some(Role::Viewer), Some(Role::Viewer)));
        assert!(delivers(Some(Role::Editor), Some(Role::Viewer)));
        assert!(delivers(Some(Role::Owner), Some(Role::Owner)));
    }

    /// Invalid: lowered below the channel's read role (a member downgrade),
    /// or the channel's read role raised past theirs (a deploy), or no
    /// member, or the channel gone: nothing goes.
    #[test]
    fn narrowed_access_stops_delivery() {
        assert!(!delivers(Some(Role::Viewer), Some(Role::Editor)), "an editor lowered to viewer on an editors' channel");
        assert!(!delivers(Some(Role::Editor), Some(Role::Owner)), "a channel raised to owner");
        assert!(!delivers(None, Some(Role::Viewer)), "no longer a member");
        for member in ROLES {
            assert!(!delivers(Some(member), None), "{member:?} on a channel no longer declared");
        }
    }

    /// Every pair: delivered exactly when the role reads the channel now.
    #[test]
    fn delivered_exactly_when_the_role_reads_now() {
        for member in ROLES {
            for read in ROLES {
                assert_eq!(delivers(Some(member), Some(read)), member >= read, "{member:?} on a channel {read:?} reads");
            }
        }
    }
}
