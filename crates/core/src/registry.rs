//! The identity registry's rules (docs/cloudflare-v1.md, R15;
//! the cell is `cell/src/registry.rs`). Who may change an identity's keys,
//! and who may look at it.

pub mod said;

use fragment_proto::IdentityKind;

/// Who manages an identity's keys (rule 4): a person manages their own; an
/// agent's are managed by its owner, never by itself.
pub fn may_manage_keys(by: &str, identity: &str, kind: IdentityKind, owner: Option<&str>) -> bool {
    match kind {
        IdentityKind::Person => by == identity,
        IdentityKind::Agent => owner == Some(by),
    }
}

/// Who sees an identity's keys and agents: itself and its owner.
pub fn may_view(by: &str, identity: &str, owner: Option<&str>) -> bool {
    by == identity || owner == Some(by)
}

/// Who may ask whether a key is one of `identity`'s: the identity itself,
/// or an agent it owns (an agent's runtime checks that a request comes
/// from its owner, whose keys may change).
pub fn may_check_key(by: &str, by_owner: Option<&str>, identity: &str) -> bool {
    by == identity || by_owner == Some(identity)
}

/// Whether the deployment's operators (`listed`, `FRAGMENT_OPERATORS`, each
/// 64 hex) name a signer: by its key (64 hex), when it signed with one, or
/// by its identity (docs/api.md, Operators).
pub fn lists_operator(listed: &[String], key: Option<&str>, identity: &str) -> bool {
    listed.iter().any(|l| Some(l.as_str()) == key || crate::npub::identity_of(l) == identity)
}

/// Whether `by`, an operator as the router names one (their identity, or
/// a key no one holds by its npub), is one: listed itself, or a person who
/// holds (`held`: their keys not revoked, 64 hex) a key that is. What an
/// actor doing an operator's work checks again, beside the router.
pub fn names_operator(listed: &[String], by: &str, held: &[String]) -> bool {
    lists_operator(listed, crate::npub::parse(by).as_deref(), by) || held.iter().any(|k| lists_operator(listed, Some(k), ""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use IdentityKind::*;

    /// The operators' list names its keys; an actor checks an operator as
    /// the router named them: an identity listed, a key no one holds by its
    /// npub, or a person by the keys they hold. No one else, and no key a
    /// person no longer holds.
    #[test]
    fn an_operator_is_named_by_the_list() {
        let (op_key, person_key, stranger_key) = ("aa".repeat(32), "bb".repeat(32), "cc".repeat(32));
        let listed = vec![op_key.clone()];
        let op = crate::npub::encode(&op_key);
        let person = crate::npub::encode(&person_key);
        assert!(lists_operator(&listed, Some(&op_key), ""), "its key signed");
        assert!(lists_operator(&listed, None, &op), "its identity");
        assert!(names_operator(&listed, &op, &[]), "a key no one holds, by its npub");
        assert!(names_operator(&listed, &person, std::slice::from_ref(&op_key)), "a person who holds a listed key");
        assert!(!names_operator(&listed, &person, std::slice::from_ref(&person_key)), "a person who holds none");
        assert!(!names_operator(&listed, &person, &[]), "a person whose listed key was revoked holds it no more");
        assert!(!names_operator(&listed, &crate::npub::encode(&stranger_key), std::slice::from_ref(&stranger_key)));
        assert!(!names_operator(&[], &op, std::slice::from_ref(&op_key)), "an empty list names no one");
        assert!(!names_operator(&listed, "", &[]) && !names_operator(&listed, "not an npub", &[]));
    }

    #[test]
    fn keys_are_managed_by_the_human() {
        assert!(may_manage_keys("id:p", "id:p", Person, None));
        assert!(!may_manage_keys("id:q", "id:p", Person, None));
        assert!(may_manage_keys("id:p", "id:a", Agent, Some("id:p")));
        // an agent never manages its own keys, and nobody else's
        assert!(!may_manage_keys("id:a", "id:a", Agent, Some("id:p")));
        assert!(!may_manage_keys("id:q", "id:a", Agent, Some("id:p")));
        assert!(!may_manage_keys("id:a", "id:p", Person, None));
    }

    #[test]
    fn who_sees_what() {
        assert!(may_view("id:p", "id:p", None));
        assert!(may_view("id:p", "id:a", Some("id:p")));
        assert!(!may_view("id:q", "id:a", Some("id:p")));
        assert!(may_check_key("id:a", Some("id:p"), "id:p"));
        assert!(may_check_key("id:p", None, "id:p"));
        assert!(!may_check_key("id:b", Some("id:q"), "id:p"));
    }
}
