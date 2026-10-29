//! Optional, first-come public addresses. A reservation belongs to one
//! fragment identity forever: deleting an app never hands its origin,
//! cookies or browser storage to another app.

use serde::{Deserialize, Serialize};

/// One DNS label. Double dashes belong to generated fragment names,
/// computers and branch marks, so these namespaces cannot collide.
pub fn valid(label: &str, mark: &str) -> bool {
    !label.is_empty()
        && label.len() + mark.len() <= 63
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
        && !label.contains("--")
}

/// The registry's immutable binding. `npub` names this life of the app,
/// so recreating its generated name cannot inherit a reserved origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub label: String,
    pub fragment: String,
    pub npub: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_disjoint_from_generated_hosts_and_fit_a_branch() {
        for label in ["a", "my-app", "123", &"a".repeat(63)] {
            assert!(valid(label, ""), "{label}");
        }
        for label in ["", "A", "-a", "a-", "a--b", "a.b", "a/b", "a%2eb", "é", &"a".repeat(64)] {
            assert!(!valid(label, ""), "{label}");
        }
        assert!(valid(&"a".repeat(54), "--preview"));
        assert!(!valid(&"a".repeat(55), "--preview"));
        assert!(!valid("todo--k3x9", ""));
    }

    /// The constraints arbitrate two simultaneous claimants, recognise a
    /// retry, and survive a reopened database. No delete releases a name.
    #[test]
    fn first_claim_wins_and_reservations_survive_restart() {
        let path = std::env::temp_dir().join(format!("fragment-address-{}-{}.sqlite", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(include_str!("../migrations/registry/0007_public_addresses.sql")).unwrap();
        let claim = "INSERT OR IGNORE INTO public_addresses (label, fragment, npub) VALUES (?, ?, ?)";
        assert_eq!(db.execute(claim, ["groceries", "todo--k3x9", "first-life"]).unwrap(), 1);
        assert_eq!(db.execute(claim, ["groceries", "todo--k3x9", "first-life"]).unwrap(), 0);
        assert_eq!(db.execute(claim, ["groceries", "other--k3x9", "other-life"]).unwrap(), 0);
        assert_eq!(db.execute(claim, ["another-name", "todo--k3x9", "first-life"]).unwrap(), 0);
        drop(db);
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(db.query_row("SELECT npub FROM public_addresses WHERE label = ?", ["groceries"], |row| row.get::<_, String>(0)).unwrap(), "first-life");
        assert_eq!(db.execute(claim, ["groceries", "todo--k3x9", "replacement-life"]).unwrap(), 0);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
