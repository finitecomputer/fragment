//! Which computer: a person's own, and the ones a page pre-wakes.

use sha2::{Digest, Sha256};

/// The id of a person's computer: one each for now (decision 13), so it is
/// derived from the owner and making it twice makes one. Later computers
/// (ephemeral ones, machines brought along) get ids of their own; nothing
/// but the person's first computer assumes this one.
pub fn default_computer_of(owner: &str) -> String {
    let digest = Sha256::digest(format!("fragment computer\0{owner}").as_bytes());
    format!("computer:{}", &hex::encode(digest)[..24])
}

/// The computers `by` beginning to type on a fragment pre-wakes (decision
/// 39): each one the fragment's wake subscriptions (`(principal,
/// computer)`) name, but none that `by`'s own name. A computer's agents
/// never pre-wake their own computer, as what they post never wakes it:
/// one as it goes to sleep would start it again from that sleep's save.
/// Each once, in the order first named.
pub fn prewoken<'a>(subs: &[(&'a str, &'a str)], by: &str) -> Vec<&'a str> {
    let own: Vec<&str> = subs.iter().filter(|(principal, _)| *principal == by).map(|(_, computer)| *computer).collect();
    let mut out: Vec<&str> = Vec::with_capacity(subs.len());
    for (_, computer) in subs {
        if !own.contains(computer) && !out.contains(computer) {
            out.push(computer);
        }
    }
    out
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

    /// Goal: a person's page pre-wakes every computer that follows the
    /// fragment, each once; a computer's own agent's socket (its guest
    /// following the fragment, as it boots or after a drop, also while it
    /// goes to sleep) pre-wakes none of its own, only other computers'.
    #[test]
    fn a_page_prewakes_the_computers_but_an_agent_never_its_own() {
        let subs = [("id:juniper", "computer:a"), ("id:juniper", "computer:a"), ("id:maple", "computer:a"), ("id:oak", "computer:b")];
        assert_eq!(prewoken(&subs, "id:paul"), ["computer:a", "computer:b"], "a person's page");
        assert_eq!(prewoken(&subs, "anon:3f"), ["computer:a", "computer:b"], "a visitor's page");
        assert_eq!(prewoken(&subs, "id:juniper"), ["computer:b"], "an agent of computer a");
        assert_eq!(prewoken(&subs, "id:maple"), ["computer:b"], "another agent of the same computer");
        assert_eq!(prewoken(&subs, "id:oak"), ["computer:a"], "an agent of computer b");
        assert!(prewoken(&[], "id:paul").is_empty(), "no computer follows it");
    }
}
