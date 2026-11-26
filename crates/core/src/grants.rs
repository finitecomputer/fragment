//! What an agent may use of its owner's (docs/computers.md, "Who may use
//! what"): the providers it may have swapped in, every one its owner has
//! unless they narrowed it to a list (decision 44), and which of their
//! accounts it uses at each connection that takes several. Its agent
//! fragment holds them (`meta` `grants`), changed only by its owner in
//! person through their computer's routes; its computer asks for them at
//! each use and keeps none. Pure: the cell stores and asks, this reads.
//!
//! **The move** from the computer, which kept them before: an agent's
//! first ask after the deploy carried its computer's row (`Row`), taken
//! whole by a fragment that holds no grants of its own yet, and ignored by
//! one that does (`answered`), so a row sent late or again can never undo
//! what its owner set since. The Computer's step 6 dropped those rows,
//! keeping each one not yet taken as owed (`Owed`, its `meta`
//! `grants_owed`), handed over the same way and forgotten once answered.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::catalog::{valid_name, PROVIDERS_MAX};
use crate::connection_accounts::valid_account_id;

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
    /// While someone else's ask is open (FIN-720: `mandate::Proof`), what
    /// of these its place lends: an answer's alone, never kept. `None`:
    /// all of them, its owner's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lend: Option<fragment_proto::Lend>,
}

impl Grants {
    /// None at all: what an agent whose fragment did not answer may use.
    pub fn none() -> Grants {
        Grants { connections: Some(vec![]), accounts: BTreeMap::new(), lend: None }
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
        assert!(self.lend.is_none(), "a lend narrows an answer, and is never kept");
        serde_json::to_string(self).expect("grants serialize")
    }
}

/// Whether an agent narrowed to `narrowed` (`None`: not narrowed) may use
/// `provider`.
pub fn allows(narrowed: Option<&[String]>, provider: &str) -> bool {
    narrowed.is_none_or(|list| list.iter().any(|p| p == provider))
}

/// A computer's row for an agent from before the move: its connections as
/// they were stored (`null`, or a list) and its picks, provider to id.
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

/// The rows a computer owes its agents' fragments (its `meta`
/// `grants_owed`, written by the Computer's step 6): agent fragment → the
/// agent it was kept for and its row. At most the agents it ran then,
/// and only ever fewer: none is written after.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Owed(pub BTreeMap<String, (String, Row)>);

impl Owed {
    /// As its computer's `meta` keeps it: at most `max` rows, its agents.
    pub fn read(stored: &str, max: usize) -> Result<Owed, String> {
        let v: Value = serde_json::from_slice(stored.as_bytes()).map_err(|e| e.to_string())?;
        let fragments = v.as_object().ok_or("owed rows are an object, by agent fragment")?;
        if fragments.len() > max {
            return Err(format!("{} rows owed, more than a computer's {max} agents", fragments.len()));
        }
        let mut owed = BTreeMap::new();
        for (fragment, o) in fragments {
            let identity = o["identity"].as_str().ok_or_else(|| format!("{fragment}'s owed row names no agent"))?;
            let row = Row::of(o).ok_or_else(|| format!("{fragment}'s owed row is no row: {o}"))?;
            owed.insert(fragment.clone(), (identity.to_string(), row));
        }
        Ok(Owed(owed))
    }

    /// As its computer's `meta` keeps it; none owed, nothing kept.
    pub fn stored(&self) -> Option<String> {
        let rows: serde_json::Map<String, Value> = self.0.iter().map(|(f, (identity, row))| (f.clone(), json!({ "identity": identity, "connections": row.connections, "accounts": row.accounts }))).collect();
        (!rows.is_empty()).then(|| Value::Object(rows).to_string())
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
        (None, Some(row)) => {
            let connections = serde_json::from_slice(row.connections.as_bytes()).map_err(|e| format!("its computer's connections {:?}: {e}", row.connections))?;
            Grants { connections, accounts: row.accounts.clone(), lend: None }
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
        let held = Grants { connections: list(&["github"]), accounts: BTreeMap::from([("google".into(), "data_1".into())]), lend: None };
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
        let picked = answered(None, None, Some(Change::Account { provider: "google".into(), account: Some("data_1".into()) })).expect("picked");
        assert_eq!((picked.connections.clone(), picked.account("google")), (None, Some("data_1")));
    }

    /// An agent's row at step 5: its fragment, its `agents.connections`
    /// and its `agent_accounts` picks, provider to account.
    type OldRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// A computer at step 5 holding each of `rows`.
    fn at_step_5(rows: &[OldRow]) -> crate::migrate::host::Lite {
        use crate::migrate::{self, host::Lite, Ran, COMPUTER};
        let db = Lite::new();
        let five = migrate::Class { steps: &COMPUTER.steps[..5], ..COMPUTER };
        assert_eq!(db.transaction(|db| migrate::migrate(db, &five, 0)), Ok(Ran::Made { version: 5 }));
        for (i, (fragment, connections, picks)) in rows.iter().enumerate() {
            db.harness(&format!("INSERT INTO agents (fragment, identity, owner, added_at, connections) VALUES ('{fragment}', 'npub1{i}', 'npub1o', {i}, '{connections}')"));
            for (provider, account) in *picks {
                db.harness(&format!("INSERT INTO agent_accounts (agent, provider, account) VALUES ('{fragment}', '{provider}', '{account}')"));
            }
        }
        db
    }

    fn owed_of(db: &crate::migrate::host::Lite) -> Option<Owed> {
        use crate::migrate::Db;
        let rows = db.rows("SELECT value FROM meta WHERE key = 'grants_owed'").expect("meta reads");
        rows.first().map(|r| Owed::read(r["value"].as_str().expect("text"), 16).expect("step 6's rows read as the cell reads them"))
    }

    /// Goal: step 6 drops the computer's old grant rows and loses none its
    /// agents' fragments have not taken: each is owed, read as the cell
    /// reads it, and taken whole as before (the upgrade path). A row taken,
    /// or one saying only the default, is not owed; narrowed to none is.
    /// Started again it changes nothing (replay, restart); the code of the
    /// step before refuses it (rollback). Method: a step-5 computer holding
    /// every kind of row, migrated by this code, then by the step before.
    #[test]
    fn step_6_owes_each_row_not_taken() {
        use crate::migrate::{self, Ran, SchemaError, COMPUTER};
        let newest = COMPUTER.steps.len() as u32;
        assert_eq!(COMPUTER.steps.get(5).map(|s| s.name), Some("grants_owed"), "the drop's step is step 6");
        let db = at_step_5(&[
            ("taken--a1", r#"["-"]"#, &[("google", "data_stale")]),
            ("narrowed--b2", r#"["github"]"#, &[("google", "data_1")]),
            ("default--c3", "null", &[]),
            ("picked--d4", "null", &[("google", "data_2")]),
            ("none--e5", "[]", &[]),
        ]);
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 100)), Ok(Ran::Migrated { from: 5, version: newest }));
        let owed = owed_of(&db).expect("rows owed");
        let expected = Owed(BTreeMap::from([
            ("narrowed--b2".to_string(), ("npub11".to_string(), row(r#"["github"]"#, &[("google", "data_1")]))),
            ("picked--d4".to_string(), ("npub13".to_string(), row("null", &[("google", "data_2")]))),
            ("none--e5".to_string(), ("npub14".to_string(), row("[]", &[]))),
        ]));
        assert_eq!(owed, expected, "each row not taken, and none other");
        let taken = |f: &str| answered(None, Some(&owed.0[f].1), None).expect("taken whole");
        assert_eq!((taken("narrowed--b2").connections, taken("narrowed--b2").account("google").map(str::to_string)), (list(&["github"]), Some("data_1".into())));
        assert_eq!((taken("picked--d4").connections, taken("picked--d4").account("google").map(str::to_string)), (None, Some("data_2".into())));
        assert_eq!(taken("none--e5"), Grants::none(), "narrowed to none stays none, never every provider");
        assert_eq!(db.columns("agents"), ["fragment", "identity", "owner", "added_at"], "the column is gone");
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'agent_accounts'"), 0, "the table is gone");
        assert_eq!(db.count("SELECT COUNT(*) FROM agents"), 5, "every agent still runs here");
        // replay and restart
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 200)), Ok(Ran::Current { version: newest }));
        assert_eq!(owed_of(&db), Some(expected), "started again, it owes the same");
        // rollback
        let before = migrate::Class { steps: &COMPUTER.steps[..5], ..COMPUTER };
        assert_eq!(db.transaction(|db| migrate::migrate(db, &before, 300)), Err(SchemaError::Newer { class: "Computer", held: newest, known: 5 }), "the release before refuses it");
        // a wipe makes its life again, as step 6 left it
        db.transaction(|db| migrate::remake(db, &COMPUTER)).expect("remade");
        assert_eq!((db.columns("agents").len(), db.count("SELECT COUNT(*) FROM meta")), (4, 0));
    }

    /// Goal: a computer whose agents' fragments took every row, or that
    /// held only defaults, owes nothing and keeps no row saying so.
    #[test]
    fn nothing_owed_keeps_no_row() {
        use crate::migrate::{self, Ran, COMPUTER};
        for rows in [&[][..], &[("taken--a1", r#"["-"]"#, &[][..]), ("default--c3", "null", &[][..])][..]] {
            let db = at_step_5(rows);
            assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 100)), Ok(Ran::Migrated { from: 5, version: COMPUTER.steps.len() as u32 }));
            assert_eq!(owed_of(&db), None, "{rows:?} owes nothing");
        }
    }

    /// Goal: what a computer owes reads back as it was kept, shrinks to
    /// nothing kept, and refuses what step 6 never writes.
    #[test]
    fn owed_rows_read_as_kept() {
        let mut owed = Owed(BTreeMap::from([("narrowed--b2".to_string(), ("npub1b".to_string(), row(r#"["github"]"#, &[("google", "data_1")])))]));
        assert_eq!(Owed::read(&owed.stored().expect("kept"), 16), Ok(owed.clone()));
        owed.0.clear();
        assert_eq!(owed.stored(), None, "none owed, nothing kept");
        let one = r#"{"connections":"null","accounts":{}}"#;
        for stored in ["[]", "null", r#"{"a--b1":{"connections":"null","accounts":{}}}"#, r#"{"a--b1":{"identity":"npub1a","accounts":{}}}"#, r#"{"a--b1":{"identity":"npub1a","connections":"null","accounts":{"google":1}}}"#] {
            assert!(Owed::read(stored, 16).is_err(), "{stored} is refused");
        }
        let many: Vec<String> = (0..3).map(|i| format!(r#""f--{i}":{{"identity":"npub1a",{}"#, &one[1..])).collect();
        assert!(Owed::read(&format!("{{{}}}", many.join(",")), 2).is_err(), "more than its agents");
        assert!(Owed::read(&format!("{{{}}}", many.join(",")), 3).is_ok());
    }
}
