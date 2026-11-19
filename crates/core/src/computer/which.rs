//! Which computer: a person's own, and the one their typing pre-wakes.

use sha2::{Digest, Sha256};

/// The id of a person's computer: one each for now (decision 13), so it is
/// derived from the owner and making it twice makes one. Later computers
/// (ephemeral ones, machines brought along) get ids of their own; nothing
/// but the person's first computer assumes this one.
pub fn default_computer_of(owner: &str) -> String {
    let digest = Sha256::digest(format!("fragment computer\0{owner}").as_bytes());
    format!("computer:{}", &hex::encode(digest)[..24])
}

/// The computer `by` beginning to type on a fragment pre-wakes (decision
/// 39): their own (`default_computer_of`), when one of the fragment's wake
/// subscriptions (`(principal, computer URL)`) names it, but never one
/// `by`'s own subscriptions name (an agent's socket wakes no computer it
/// runs on). Anyone else's typing wakes nothing, whatever they may read or
/// post there: their message wakes it when posted, and its awake time is
/// its owner's to pay.
pub fn prewoken<'a>(subs: &[(&'a str, &'a str)], by: &str) -> Option<&'a str> {
    let own = default_computer_of(by);
    let names = |url: &str| url.trim_start_matches("computer:") == own.trim_start_matches("computer:");
    if subs.iter().any(|(principal, url)| *principal == by && names(url)) {
        return None;
    }
    subs.iter().map(|(_, url)| *url).find(|url| names(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_persons_computer_has_one_stable_id() {
        let a = default_computer_of("id:aaaa");
        assert_eq!(a, default_computer_of("id:aaaa"));
        assert_ne!(a, default_computer_of("id:aaab"));
        assert!(a.starts_with("computer:") && a.len() == "computer:".len() + 24 && a[9..].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// Goal (review of #1351): typing pre-wakes only the typer's own
    /// computer, never anyone else's, whatever they may read or post
    /// there. Valid (its owner, among other owners' computers), invalid (an
    /// anonymous reader of a public guestbook, an invited member, the
    /// owner of a computer the fragment does not follow), and an agent's
    /// own socket wakes none.
    #[test]
    fn only_its_owners_typing_prewakes_a_computer() {
        let (paul, ann) = (default_computer_of("npub1paul"), default_computer_of("npub1ann"));
        // a chat (or a public guestbook with an editors' channel) followed
        // by two people's agents, each from its owner's computer
        let subs = [("npub1juniper", paul.as_str()), ("npub1juniper", paul.as_str()), ("npub1maple", ann.as_str())];
        let woken = |by: &str| prewoken(&subs, by).into_iter().collect::<Vec<_>>();
        assert_eq!(woken("npub1paul"), [paul.as_str()], "its owner: their own computer, once, never another's");
        assert_eq!(woken("npub1ann"), [ann.as_str()], "the other owner: theirs alone");
        assert!(woken("anon:3f").is_empty(), "an anonymous reader (a public guestbook's)");
        assert!(woken("npub1bob").is_empty(), "an invited member who owns none of them");
        assert!(woken("npub1juniper").is_empty() && woken("npub1maple").is_empty(), "an agent's own socket");
        assert!(prewoken(&[("npub1maple", ann.as_str())], "npub1paul").into_iter().next().is_none(), "an owner whose computer does not follow it");
        // a subscription names its computer as the URL `computer:<id>` (runs_on.rs)
        let url = format!("computer:{}", paul.trim_start_matches("computer:"));
        assert_eq!(prewoken(&[("npub1juniper", url.as_str())], "npub1paul").into_iter().collect::<Vec<_>>(), [url.as_str()]);
    }
}
