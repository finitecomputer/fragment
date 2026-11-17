//! What an agent may use of its owner's (docs/computers.md, "Who may use
//! what"): the providers it may have swapped in, every one its owner has
//! unless they narrowed it to a list (decision 44), and which of their
//! accounts it uses at each connection that takes several. Its agent
//! fragment holds them (`meta` `grants`), changed only by its owner in
//! person through their computer's routes; its computer asks for them at
//! each use and keeps none. Pure: the cell stores and asks, this reads.
//!
//! **The move** from the computer, which kept them before (its
//! `agents.connections` and `agent_accounts`, the release before's only
//! record): a computer sends its agent's row as it stands (`Row`) with
//! each ask, then marks its `connections` `MOVED`. The fragment keeps the
//! row it saw last and takes from a row only what changed since
//! (`answered`): the whole of it the first time; after that, what the
//! release before wrote during a rollback (a list, an account picked or
//! reset), field by field. A row sent again unchanged (a reply lost after
//! the fragment took it, a stop before the mark) changes nothing, so it
//! never undoes what its owner set since. This release never writes the
//! old picks table but to clear another life's rows. The release before
//! reads the mark as a list naming no provider and never writes it, so a
//! rollback refuses a moved agent every provider rather than widen it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

/// The SQL a computer reads its agent's row with (`?`: the agent fragment),
/// and marks it moved with (`?`: `MOVED`, the fragment): the cell's, and
/// the compatibility tests' beside the release before's own writes.
pub const ROW_CONNECTIONS: &str = "SELECT connections FROM agents WHERE fragment = ?";
pub const ROW_ACCOUNTS: &str = "SELECT provider, account FROM agent_accounts WHERE agent = ?";
pub const MARK: &str = "UPDATE agents SET connections = ? WHERE fragment = ?";

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

/// What an agent's fragment holds after its computer's ask: what it held,
/// with what its computer's `row` changed since the row it `seen` last
/// (all of it, the first time), then its owner's `change`. A `MOVED`
/// row's `connections` change nothing: this release marked them. Refused
/// when the row does not read, or the result is no agent's grants.
pub fn answered(held: Option<&Grants>, seen: Option<&Row>, row: Option<&Row>, change: Option<Change>) -> Result<Grants, String> {
    let mut grants = held.cloned().unwrap_or_default();
    if let Some(row) = row {
        if row.connections != MOVED && seen.is_none_or(|s| s.connections != row.connections) {
            grants.connections = serde_json::from_slice(row.connections.as_bytes()).map_err(|e| format!("its computer's connections {:?}: {e}", row.connections))?;
        }
        let before = seen.map(|s| &s.accounts);
        let named = row.accounts.keys().chain(before.into_iter().flat_map(|b| b.keys()));
        let changed: Vec<String> = named.filter(|p| before.is_none_or(|b| b.get(*p) != row.accounts.get(*p))).cloned().collect();
        for provider in changed {
            match row.accounts.get(&provider) {
                Some(account) => grants.accounts.insert(provider, account.clone()),
                None => grants.accounts.remove(&provider),
            };
        }
    }
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

    #[test]
    fn moved_is_no_provider_and_never_written_before() {
        // the release before reads the mark as a narrowed list (its
        // `may_use`): no catalog provider is named `-`, so it refuses all
        let marked: Vec<String> = serde_json::from_str(MOVED).expect("a list");
        assert!(marked.iter().all(|n| !valid_name(n)), "the release before refuses this name in a write");
        assert!(!Grants { connections: Some(marked), ..Grants::default() }.may_use("google"));
    }

    fn row(connections: &str, accounts: &[(&str, &str)]) -> Row {
        Row { connections: connections.into(), accounts: accounts.iter().map(|(p, a)| (p.to_string(), a.to_string())).collect() }
    }

    #[test]
    fn a_row_is_taken_whole_the_first_time() {
        let taken = answered(None, None, Some(&row(r#"["github"]"#, &[("google", "data_1")])), None).expect("reads");
        assert_eq!(taken.connections, list(&["github"]));
        assert_eq!(taken.account("google"), Some("data_1"));
        assert_eq!(answered(None, None, Some(&row("[]", &[])), None), Ok(Grants::none()), "narrowed to none is none, never every");
        assert_eq!(answered(None, None, Some(&row(MOVED, &[])), None), Ok(Grants::default()), "a row this release made says nothing");
        assert!(answered(None, None, Some(&row("{}", &[])), None).is_err(), "a row that does not read is a fault, never every provider");
        assert_eq!(Row::of(&row(MOVED, &[("google", "data_1")]).value()), Some(row(MOVED, &[("google", "data_1")])));
    }

    #[test]
    fn a_row_sent_again_unchanged_changes_nothing() {
        let first = row("null", &[]);
        // its owner narrowed to none as the row was first sent; the reply
        // was lost, so its computer never marked it, and sends it again
        let held = answered(None, None, Some(&first), Some(Change::Connections(Some(vec![])))).expect("taken");
        assert_eq!(held, Grants::none());
        assert_eq!(answered(Some(&held), Some(&first), Some(&first), None), Ok(Grants::none()), "never widened back to every");
    }

    #[test]
    fn a_rollbacks_edits_are_taken_field_by_field() {
        let seen = row(MOVED, &[("e2e-several", "data_a")]);
        let held = Grants { connections: list(&["e2e-several"]), accounts: BTreeMap::from([("e2e-several".into(), "data_a".into())]) };
        let after = |r: Row| answered(Some(&held), Some(&seen), Some(&r), None).expect("reads");
        assert_eq!(after(row(MOVED, &[("e2e-several", "data_b")])).account("e2e-several"), Some("data_b"), "A to B");
        assert_eq!(after(row(MOVED, &[])).account("e2e-several"), None, "A to the default");
        let listed = after(row(r#"["e2e-several","github"]"#, &[("e2e-several", "data_a")]));
        assert_eq!((listed.connections, listed.accounts), (list(&["e2e-several", "github"]), held.accounts.clone()), "a list alone keeps the pick");
        assert_eq!(after(seen.clone()), held, "nothing written, nothing changed");
    }
}

/// The release before's own writes (ff82fb88e, cell/src/computer.rs,
/// verbatim) on a real SQLite holding the Computer's schema, against this
/// release's computer (its reads and its mark: `ROW_CONNECTIONS`,
/// `ROW_ACCOUNTS`, `MARK`) and its agent's fragment (`answered`, keeping
/// what it holds and the row it saw last, as runs_on.rs does).
#[cfg(all(test, feature = "sqlite"))]
mod compat {
    use super::*;
    use rusqlite::{params, Connection};

    const AGENT: &str = "juniper--k3x9";
    const SEVERAL: &str = "e2e-several";

    fn computer() -> Connection {
        let db = Connection::open_in_memory().expect("an in-memory SQLite");
        for step in crate::migrate::COMPUTER.steps {
            db.execute_batch(step.sql).expect(step.name);
        }
        db
    }

    /// The release before, as its routes write: assign, a list, an
    /// account picked or reset, and an unassign.
    struct Before<'a>(&'a Connection);

    impl Before<'_> {
        fn assign(&self) {
            let sql = "INSERT INTO agents (fragment, identity, owner, added_at, connections) VALUES (?, ?, ?, ?, 'null') ON CONFLICT (fragment) DO UPDATE SET identity = excluded.identity";
            self.0.execute(sql, params![AGENT, "npub1agent", "npub1owner", 1]).expect("assign");
        }

        fn connections(&self, list: Option<&[&str]>) {
            let text = serde_json::to_string(&list).expect("a list");
            self.0.execute("UPDATE agents SET connections = ? WHERE fragment = ?", params![text, AGENT]).expect("connections");
        }

        fn account(&self, account: Option<&str>) {
            match account {
                Some(account) => self.0.execute(
                    "INSERT INTO agent_accounts (agent, provider, account) VALUES (?, ?, ?) ON CONFLICT (agent, provider) DO UPDATE SET account = excluded.account",
                    params![AGENT, SEVERAL, account],
                ),
                None => self.0.execute("DELETE FROM agent_accounts WHERE agent = ? AND provider = ?", params![AGENT, SEVERAL]),
            }
            .expect("account");
        }

        fn unassign(&self) {
            let gone: Vec<String> = self.0.prepare("DELETE FROM agents WHERE fragment = ? RETURNING fragment").and_then(|mut q| q.query_map(params![AGENT], |r| r.get(0))?.collect()).expect("unassign");
            if !gone.is_empty() {
                self.0.execute("DELETE FROM agent_accounts WHERE agent = ?", params![AGENT]).expect("its picks");
            }
        }
    }

    /// This release: its computer's ask, and its agent's fragment.
    struct Now<'a> {
        db: &'a Connection,
        held: Option<Grants>,
        seen: Option<Row>,
    }

    impl Now<'_> {
        fn new(db: &Connection) -> Now<'_> {
            Now { db, held: None, seen: None }
        }

        fn row(&self) -> Option<Row> {
            let connections: Option<String> = self.db.query_row(ROW_CONNECTIONS, params![AGENT], |r| r.get(0)).ok();
            let mut q = self.db.prepare(ROW_ACCOUNTS).expect("its picks");
            let picks: Vec<(String, String)> = q.query_map(params![AGENT], |r| Ok((r.get(0)?, r.get(1)?))).expect("read").collect::<Result<_, _>>().expect("rows");
            connections.map(|connections| Row { connections, accounts: picks.into_iter().collect() })
        }

        /// One ask, its owner's `change` with it; `lost`: its reply lost
        /// once the fragment answered, so the computer marks nothing.
        fn ask(&mut self, change: Option<Change>, lost: bool) -> Grants {
            let row = self.row();
            let grants = answered(self.held.as_ref(), self.seen.as_ref(), row.as_ref(), change).expect("answered");
            self.held = Some(grants.clone());
            if let Some(row) = &row {
                self.seen = Some(row.clone());
                if !lost && row.connections != MOVED {
                    self.db.execute(MARK, params![MOVED, AGENT]).expect("marked");
                }
            }
            grants
        }
    }

    /// Picked A on the release before, then this release's first ask.
    fn moved_with_a(db: &Connection) -> Now<'_> {
        let before = Before(db);
        before.assign();
        before.account(Some("data_a"));
        let mut now = Now::new(db);
        assert_eq!(now.ask(None, false).account(SEVERAL), Some("data_a"), "its pick taken with its row");
        assert_eq!(now.row().map(|r| r.connections), Some(MOVED.to_string()), "and the row marked");
        now
    }

    #[test]
    fn a_pick_changed_during_a_rollback_is_kept() {
        let db = computer();
        let mut now = moved_with_a(&db);
        Before(&db).account(Some("data_b"));
        assert_eq!(now.ask(None, false).account(SEVERAL), Some("data_b"), "A to B there is B here");
    }

    #[test]
    fn a_pick_reset_during_a_rollback_is_kept() {
        let db = computer();
        let mut now = moved_with_a(&db);
        Before(&db).account(None);
        assert_eq!(now.ask(None, false).account(SEVERAL), None, "A reset there is the default here, never A again");
    }

    #[test]
    fn a_list_alone_during_a_rollback_keeps_the_pick() {
        let db = computer();
        let mut now = moved_with_a(&db);
        Before(&db).connections(Some(&["e2e-several", "github"]));
        let after = now.ask(None, false);
        assert_eq!(after.account(SEVERAL), Some("data_a"), "its pick kept");
        assert_eq!(after.connections, Some(vec!["e2e-several".to_string(), "github".to_string()]), "its list taken");
    }

    #[test]
    fn a_reply_lost_after_the_fragment_took_a_narrowing_never_widens() {
        let db = computer();
        Before(&db).assign();
        let mut now = Now::new(&db);
        let narrowed = now.ask(Some(Change::Connections(Some(vec![]))), true);
        assert_eq!(narrowed, Grants::none());
        assert_ne!(now.row().map(|r| r.connections), Some(MOVED.to_string()), "its reply lost, the row stays unmarked");
        assert_eq!(now.ask(None, false), Grants::none(), "the row sent again changes nothing: never every provider");
    }

    #[test]
    fn this_releases_choices_outlive_a_rollback_that_writes_nothing() {
        let db = computer();
        let mut now = moved_with_a(&db);
        now.ask(Some(Change::Account { provider: SEVERAL.into(), account: Some("data_c".into()) }), false);
        let narrowed = now.ask(Some(Change::Connections(Some(vec![SEVERAL.into()]))), false);
        assert_eq!(now.ask(None, false), narrowed, "the row unchanged, as the release before left it");
    }

    #[test]
    fn a_reassign_during_a_rollback_is_the_release_befores_fresh_agent() {
        let db = computer();
        let mut now = moved_with_a(&db);
        now.ask(Some(Change::Connections(Some(vec![SEVERAL.into()]))), false);
        let before = Before(&db);
        before.unassign();
        before.assign();
        let after = now.ask(None, false);
        assert_eq!((after.connections.clone(), after.account(SEVERAL)), (None, None), "every provider and the default, as that release made it");
    }
}
