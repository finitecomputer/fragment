//! Which computer: a person's own.

use sha2::{Digest, Sha256};

/// The id of a person's computer: one each for now (decision 13), so it is
/// derived from the owner and making it twice makes one. Later computers
/// (ephemeral ones, machines brought along) get ids of their own; nothing
/// but the person's first computer assumes this one.
pub fn default_computer_of(owner: &str) -> String {
    let digest = Sha256::digest(format!("fragment computer\0{owner}").as_bytes());
    format!("computer:{}", &hex::encode(digest)[..24])
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
}
