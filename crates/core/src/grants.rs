//! What an agent may use of its owner's (docs/computers.md, "Who may use
//! what"): the providers it may have swapped in, every one its owner has
//! unless they narrowed it to a list (decision 44), and which of their
//! accounts it uses at each connection that takes several. Its agent
//! fragment holds them (`meta` `grants`), changed only by its owner in
//! person through their computer's routes; its computer asks for them at
//! each use and keeps none. Pure: the cell stores and asks, this reads.
//!
//! **The move** from the computer, which kept them before (its
//! `agents.connections` and `agent_accounts`): a computer hands each
//! agent's row to its fragment (`legacy`), then marks the row `MOVED`. The
//! fragment takes a hand-over whole only while it holds none of its own
//! (`handed_over`); after that, a row handed over again (one replayed
//! after a crash before the mark, or one the release before wrote during
//! a rollback) can only narrow its providers, never its accounts. The
//! release before reads the mark as a list naming no provider and never
//! writes it, so a rollback refuses a moved agent every provider rather
//! than widen it, and offers no switch and no account pick for it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::catalog::{valid_name, PROVIDERS_MAX};
use crate::connection_accounts::valid_account_id;

/// A moved row's `agents.connections`: a list whose one name no provider
/// can have (`catalog::valid_name` refuses a leading `-`).
pub const MOVED: &str = r#"["-"]"#;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    /// `None`: every provider its owner has; a list narrows it, sorted.
    /// Named even when `null`: a row that forgot it is not every provider.
    #[serde(deserialize_with = "Option::deserialize")]
    pub connections: Option<Vec<String>>,
    /// Provider → the account's id, for each connection its owner picked
    /// one at; none named, the default (`connection_accounts::chosen`).
    #[serde(default)]
    pub accounts: BTreeMap<String, String>,
}

impl Grants {
    /// None at all: what an agent whose fragment did not answer may use.
    pub fn none() -> Grants {
        Grants { connections: Some(vec![]), accounts: BTreeMap::new() }
    }

    /// Whether it may use `provider`.
    pub fn may_use(&self, provider: &str) -> bool {
        allows(self.connections.as_deref(), provider)
    }

    /// The account its owner picked for it at `provider`.
    pub fn account(&self, provider: &str) -> Option<&str> {
        self.accounts.get(provider).map(String::as_str)
    }

    /// Whether it is one an agent fragment may hold: at most `PROVIDERS_MAX`
    /// provider names each, sorted and once, each account a Pipes id.
    pub fn valid(&self) -> bool {
        let listed = self.connections.as_ref().is_none_or(|l| l.len() <= PROVIDERS_MAX && l.iter().all(|n| valid_name(n)) && l.windows(2).all(|w| w[0] < w[1]));
        listed && self.accounts.len() <= PROVIDERS_MAX && self.accounts.iter().all(|(p, a)| valid_name(p) && valid_account_id(a))
    }

    /// As its fragment's `meta` keeps it.
    pub fn read(stored: &str) -> Result<Grants, String> {
        // from its bytes: the cell's Wasm keeps one decoder a type (cell/clippy.toml)
        let grants: Grants = serde_json::from_slice(stored.as_bytes()).map_err(|e| e.to_string())?;
        if !grants.valid() {
            return Err(format!("{stored:?} names more than {PROVIDERS_MAX} providers, or one twice, or out of order, or an account that is no Pipes id"));
        }
        Ok(grants)
    }

    pub fn stored(&self) -> String {
        assert!(self.valid(), "only valid grants are kept");
        serde_json::to_string(self).expect("grants serialize")
    }
}

/// Whether an agent narrowed to `narrowed` (`None`: not narrowed) may use
/// `provider`.
pub fn allows(narrowed: Option<&[String]>, provider: &str) -> bool {
    narrowed.is_none_or(|list| list.iter().any(|p| p == provider))
}

/// A computer's row for an agent, as the release before kept it: its
/// `agents.connections` and its `agent_accounts` (provider, account). The
/// grants it says, or none once moved.
pub fn legacy(connections: &str, accounts: impl IntoIterator<Item = (String, String)>) -> Result<Option<Grants>, String> {
    if connections == MOVED {
        return Ok(None);
    }
    let connections: Option<Vec<String>> = serde_json::from_slice(connections.as_bytes()).map_err(|e| format!("its stored connections {connections:?}: {e}"))?;
    // inserted, not collected: a collect sorts its pairs first, and a
    // sort of string pairs is about 3 KB of the cell's Wasm a copy
    let mut picks = BTreeMap::new();
    for (provider, account) in accounts {
        picks.insert(provider, account);
    }
    let grants = Grants { connections, accounts: picks };
    if !grants.valid() {
        return Err(format!("its stored row {grants:?} is no agent's grants"));
    }
    Ok(Some(grants))
}

/// What a fragment holds once a computer hands it `taken` (its row from
/// before the move), when it held `held`: the row whole, while it held
/// none; else what it held, its providers narrowed to those the row also
/// names. A hand-over replayed after the fragment took it (its computer
/// stopped before marking its row moved), or a row the release before
/// wrote after the move, never widens what it holds, and never changes
/// which account it uses: its owner's picks are its own.
pub fn handed_over(held: Option<&Grants>, taken: Grants) -> Grants {
    let Some(held) = held else { return taken };
    let connections = match (&held.connections, taken.connections) {
        (None, row) => row,
        (Some(own), None) => Some(own.clone()),
        (Some(own), Some(row)) => Some(own.iter().filter(|p| row.contains(p)).cloned().collect()),
    };
    Grants { connections, accounts: held.accounts.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Option<Vec<String>> {
        Some(names.iter().map(|n| n.to_string()).collect())
    }

    #[test]
    fn every_provider_unless_narrowed() {
        let every = Grants::default();
        assert!(every.may_use("google") && every.may_use("github"));
        let narrowed = Grants { connections: list(&["github"]), ..Grants::default() };
        assert!(narrowed.may_use("github") && !narrowed.may_use("google"));
        assert!(!Grants::none().may_use("github"));
    }

    #[test]
    fn kept_as_written() {
        let mut g = Grants { connections: list(&["github", "google"]), ..Grants::default() };
        g.accounts.insert("google".into(), "data_installation_01".into());
        assert_eq!(Grants::read(&g.stored()), Ok(g.clone()));
        assert_eq!(g.account("google"), Some("data_installation_01"));
        assert_eq!(g.account("github"), None);
        assert_eq!(Grants::read(&Grants::default().stored()), Ok(Grants::default()));
    }

    #[test]
    fn invalid_ones_are_refused() {
        for stored in [
            r#"{"connections":["google","github"],"accounts":{}}"#,
            r#"{"connections":["github","github"],"accounts":{}}"#,
            r#"{"connections":["-"],"accounts":{}}"#,
            r#"{"connections":null,"accounts":{"google":"not an id"}}"#,
            r#"{"connections":null,"accounts":{"Google":"data_1"}}"#,
            r#"{"accounts":{}}"#,
            r#"{"connections":null,"accounts":{},"more":1}"#,
            "null",
        ] {
            assert!(Grants::read(stored).is_err(), "{stored} is refused");
        }
        let many: Vec<String> = (0..=PROVIDERS_MAX).map(|i| format!("p{i:03}")).collect();
        assert!(!Grants { connections: Some(many), ..Grants::default() }.valid());
    }

    #[test]
    fn moved_is_no_provider_and_never_written_before() {
        // the release before reads the mark as a narrowed list (its
        // `may_use`): no catalog provider is named `-`, so it refuses all
        let marked: Vec<String> = serde_json::from_str(MOVED).expect("a list");
        assert!(marked.iter().all(|n| !valid_name(n)), "the release before refuses this name in a write");
        assert!(!Grants { connections: Some(marked), ..Grants::default() }.may_use("google"));
        assert_eq!(legacy(MOVED, vec![("google".into(), "data_1".into())]), Ok(None), "a moved row hands nothing over, leftovers or not");
    }

    #[test]
    fn a_row_before_the_move_is_taken_whole() {
        assert_eq!(legacy("null", vec![]), Ok(Some(Grants::default())));
        let taken = legacy(r#"["github"]"#, vec![("google".into(), "data_1".into())]).expect("reads");
        let mut want = Grants { connections: list(&["github"]), ..Grants::default() };
        want.accounts.insert("google".into(), "data_1".into());
        assert_eq!(taken, Some(want));
        assert_eq!(legacy("[]", vec![]), Ok(Some(Grants::none())), "narrowed to none is kept as none, never every");
        assert!(legacy("", vec![]).is_err() && legacy("{}", vec![]).is_err(), "a row that does not read is a fault, never every provider");
    }

    #[test]
    fn a_hand_over_is_taken_whole_once() {
        let row = Grants { connections: list(&["github"]), accounts: BTreeMap::from([("google".into(), "data_1".into())]) };
        assert_eq!(handed_over(None, row.clone()), row, "a fragment holding none takes the row whole");
        let replayed = handed_over(Some(&row), row.clone());
        assert_eq!(replayed, row, "the same row again changes nothing");
    }

    #[test]
    fn a_replay_never_widens_what_its_owner_set_after() {
        // its owner narrowed to none after the hand-over; the computer
        // stopped before marking its row, which still says every provider
        let narrowed = Grants { connections: Some(vec![]), ..Grants::default() };
        assert_eq!(handed_over(Some(&narrowed), Grants::default()), narrowed);
        let some = Grants { connections: list(&["github", "google"]), ..Grants::default() };
        assert_eq!(handed_over(Some(&some), Grants { connections: list(&["github", "slack"]), ..Grants::default() }).connections, list(&["github"]), "only narrowed");
        assert_eq!(handed_over(Some(&Grants::default()), some.clone()).connections, some.connections, "every provider, narrowed to the row's list");
    }

    #[test]
    fn a_row_written_during_a_rollback_never_changes_an_account() {
        let held = Grants { connections: None, accounts: BTreeMap::from([("e2e-several".into(), "data_kept".into())]) };
        // a list alone (its picks gone, as the move left the old table)
        let list_only = Grants { connections: list(&["e2e-several", "google"]), ..Grants::default() };
        assert_eq!(handed_over(Some(&held), list_only).accounts, held.accounts, "a list written alone keeps the picks");
        // another account picked, or the pick reset, with a list
        let other = Grants { connections: None, accounts: BTreeMap::from([("e2e-several".into(), "data_other".into())]) };
        assert_eq!(handed_over(Some(&held), other).accounts, held.accounts);
        assert_eq!(handed_over(Some(&held), Grants::default()).accounts, held.accounts);
    }
}
