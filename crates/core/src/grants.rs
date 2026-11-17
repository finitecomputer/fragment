//! What an agent may use of its owner's (docs/computers.md, "Who may use
//! what"): the providers it may have swapped in, every one its owner has
//! unless they narrowed it to a list (decision 44), and which of their
//! accounts it uses at each connection that takes several. Its agent
//! fragment holds them (`meta` `grants`), changed only by its owner in
//! person through their computer's routes; its computer asks for them at
//! each use and keeps none. Pure: the cell stores and asks, this reads.
//!
//! **The move** from the computer, which kept them before (its
//! `agents.connections` and `agent_accounts`): an agent's first ask after
//! the deploy carries that row (`Row`), taken whole by a fragment that
//! holds no grants of its own yet, and ignored by one that does
//! (`answered`), so a row sent late or again can never undo what its owner
//! set since. The computer then marks the row `MOVED` and sends it no
//! more. The release is forward only (the Computer's step 5): the release
//! before refuses every Computer object, so it never writes those rows
//! again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::catalog::{valid_name, PROVIDERS_MAX};
use crate::connection_accounts::valid_account_id;

/// A taken row's `agents.connections`: its fragment holds it, so its
/// computer sends it no more. A list whose one name no provider can have
/// (`catalog::valid_name` refuses a leading `-`), never a value of its own.
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

/// A computer's row for an agent as it stands: its `agents.connections` as
/// stored (`MOVED` once marked) and its `agent_accounts`, provider to id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub connections: String,
    pub accounts: BTreeMap<String, String>,
}

impl Row {
    /// As a call, and its fragment's `meta`, carry it.
    pub fn value(&self) -> Value {
        json!({ "connections": self.connections, "accounts": self.accounts })
    }

    pub fn of(v: &Value) -> Option<Row> {
        let mut accounts = BTreeMap::new();
        for (provider, account) in v["accounts"].as_object()? {
            accounts.insert(provider.clone(), account.as_str()?.to_string());
        }
        Some(Row { connections: v["connections"].as_str()?.to_string(), accounts })
    }
}

/// Its owner's change, in person: its providers, or the account it uses at
/// one (`None`: the default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Connections(Option<Vec<String>>),
    Account { provider: String, account: Option<String> },
}

/// What an agent's fragment holds after its computer's ask: what it held;
/// else, holding none of its own, the row its computer kept before the move
/// (`taken`), whole; else the default (every provider, each connection's
/// default account); then its owner's `change`. Refused when the row does
/// not read, or the result is no agent's grants.
pub fn answered(held: Option<&Grants>, taken: Option<&Row>, change: Option<Change>) -> Result<Grants, String> {
    let mut grants = match (held, taken) {
        (Some(held), _) => held.clone(),
        (None, Some(row)) if row.connections != MOVED => {
            let connections = serde_json::from_slice(row.connections.as_bytes()).map_err(|e| format!("its computer's connections {:?}: {e}", row.connections))?;
            Grants { connections, accounts: row.accounts.clone() }
        }
        (None, _) => Grants::default(),
    };
    match change {
        Some(Change::Connections(list)) => grants.connections = list,
        Some(Change::Account { provider, account: Some(account) }) => {
            grants.accounts.insert(provider, account);
        }
        Some(Change::Account { provider, account: None }) => {
            grants.accounts.remove(&provider);
        }
        None => {}
    }
    if !grants.valid() {
        return Err(format!("{grants:?} is no agent's grants"));
    }
    Ok(grants)
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

    fn row(connections: &str, accounts: &[(&str, &str)]) -> Row {
        Row { connections: connections.into(), accounts: accounts.iter().map(|(p, a)| (p.to_string(), a.to_string())).collect() }
    }

    #[test]
    fn the_first_ask_takes_the_old_row_whole() {
        let taken = answered(None, Some(&row(r#"["github"]"#, &[("google", "data_1")])), None).expect("reads");
        assert_eq!((taken.connections.clone(), taken.account("google")), (list(&["github"]), Some("data_1")));
        assert_eq!(answered(None, Some(&row("[]", &[])), None), Ok(Grants::none()), "narrowed to none is none, never every");
        assert!(answered(None, Some(&row("{}", &[])), None).is_err(), "a row that does not read is a fault, never every provider");
        assert_eq!(Row::of(&row("null", &[("google", "data_1")]).value()), Some(row("null", &[("google", "data_1")])));
    }

    #[test]
    fn a_later_row_is_ignored() {
        let held = Grants { connections: list(&["github"]), accounts: BTreeMap::from([("google".into(), "data_1".into())]) };
        for later in [row("null", &[]), row(r#"["github","slack"]"#, &[("google", "data_2")]), row("[]", &[])] {
            assert_eq!(answered(Some(&held), Some(&later), None), Ok(held.clone()), "{later:?} changes nothing");
        }
    }

    #[test]
    fn a_narrowing_survives_a_delayed_pre_take_ask() {
        // Codex round 2 P1: an ask read the old row (every provider) before
        // the narrowing took it, and arrives after (a release refusal's retry)
        let legacy = row("null", &[]);
        let narrowed = answered(None, Some(&legacy), Some(Change::Connections(Some(vec![])))).expect("taken, then narrowed");
        assert_eq!(narrowed, Grants::none());
        let viewed = answered(Some(&narrowed), None, None).expect("an ask once marked carries no row");
        assert_eq!(answered(Some(&viewed), Some(&legacy), None), Ok(Grants::none()), "the delayed ask never widens");
        // and a reply lost: the same row sent again
        assert_eq!(answered(Some(&narrowed), Some(&legacy), None), Ok(Grants::none()));
    }

    #[test]
    fn an_agent_with_no_old_row_starts_fresh() {
        assert_eq!(answered(None, None, None), Ok(Grants::default()), "every provider, each connection's default account");
        assert_eq!(answered(None, Some(&row(MOVED, &[("google", "data_1")])), None), Ok(Grants::default()), "a row marked taken is no row");
        let picked = answered(None, None, Some(Change::Account { provider: "google".into(), account: Some("data_1".into()) })).expect("picked");
        assert_eq!((picked.connections.clone(), picked.account("google")), (None, Some("data_1")));
    }

    /// Goal: the release before can never write an agent's old row again,
    /// so nothing it writes needs merging (docs/MODEL.md: a release whose
    /// stored value an older one cannot read is forward only). An object
    /// that took Computer step 5 is refused by the code of the step before,
    /// so rollout's `eligible` never rolls back across it; rolled forward,
    /// it is current (valid, rollback, restart). Method: a Computer made by
    /// this code, then started by the code of one step before, then by this.
    #[test]
    fn the_code_one_step_before_refuses_the_computer() {
        use crate::migrate::{self, host::Lite, Db, Ran, SchemaError, COMPUTER};
        let newest = COMPUTER.steps.len() as u32;
        assert_eq!((newest, COMPUTER.steps.last().map(|s| s.name)), (5, Some("grants_forward_only")), "the move's step is the newest");
        let before = migrate::Class { steps: &COMPUTER.steps[..4], ..COMPUTER };
        let db = Lite::new();
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 0)), Ok(Ran::Made { version: newest }));
        db.exec("INSERT INTO agents (fragment, identity, owner, added_at, connections) VALUES ('juniper--k3x9', 'npub1a', 'npub1o', 1, '[\"-\"]')").expect("a taken row");
        assert_eq!(db.transaction(|db| migrate::migrate(db, &before, 100)), Err(SchemaError::Newer { class: "Computer", held: newest, known: 4 }), "the release before refuses it");
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 200)), Ok(Ran::Current { version: newest }), "rolled forward, it is current");
        assert_eq!(db.rows("SELECT connections FROM agents").expect("its row")[0]["connections"], MOVED, "its row as written");
    }
}
