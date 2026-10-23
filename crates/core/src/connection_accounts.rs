//! Several accounts at one connection (its row's `accounts_max` above 1;
//! docs/pipes-browser-binding.md, Several accounts): Pipes' plural list,
//! read in order with its default account, which account an agent's swap
//! uses, what a callback imports (a first account, another one, or one
//! connected before, reauthorized), and what Pipes' answer to an import
//! means. Pure: the cell asks Pipes, and this decides.

use fragment_proto::computer::ProviderState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::connection_signin::{label_ok, pipes_scopes};

/// A connected account's id at Pipes (`data_installation_…`), at most.
pub const ACCOUNT_ID_MAX_BYTES: usize = 64;
/// An account's identifier (`account_identifier`: the ID token's `sub`,
/// which the import sets), at most.
pub const IDENTIFIER_MAX_BYTES: usize = 255;

/// Whether `id` is a connected account's id as Pipes makes them: 1 to
/// `ACCOUNT_ID_MAX_BYTES` letters, digits and `_`. It goes in a query and
/// in a computer's table, and only one Pipes listed is ever used.
pub fn valid_account_id(id: &str) -> bool {
    (1..=ACCOUNT_ID_MAX_BYTES).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whether `identifier` is one an account may be told apart by: printable,
/// no space, bounded (Google's `sub` is digits).
pub fn identifier_ok(identifier: &str) -> bool {
    (1..=IDENTIFIER_MAX_BYTES).contains(&identifier.len()) && identifier.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// A connected account as Pipes describes it, with no token: its id (none
/// where it was read as the compatibility connection, which needs none),
/// its identifier (the provider account it is: `connection_signin::
/// account_identifier`, set by the import), its label as its person knows
/// it (`account_display_name`: `connection_signin::account_label`), its
/// state, the scopes it holds (`connection_signin::pipes_scopes`; None
/// when it names none), and when it was made. `fenced`: one the person
/// disconnected that Pipes holds still (`fenced`), not theirs to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipesAccount {
    pub id: Option<String>,
    pub identifier: Option<String>,
    pub label: Option<String>,
    pub state: ProviderState,
    pub scopes: Option<Vec<String>>,
    pub created_at: String,
    pub fenced: bool,
}

impl PipesAccount {
    /// No account: not connected, naming nothing.
    pub fn none() -> PipesAccount {
        PipesAccount { id: None, identifier: None, label: None, state: ProviderState::NotConnected, scopes: None, created_at: String::new(), fenced: false }
    }
}

/// A connected account object of Pipes' (one `GET` answers, or one of the
/// plural list's), read: a state it does not know is not connected, which
/// names no label and no scopes.
pub fn described(account: &Value) -> PipesAccount {
    let state = match account["state"].as_str() {
        Some("connected") => ProviderState::Connected,
        Some("needs_reauthorization") => ProviderState::NeedsReauthorization,
        _ => ProviderState::NotConnected,
    };
    let held = state != ProviderState::NotConnected;
    PipesAccount {
        id: account["id"].as_str().filter(|i| valid_account_id(i)).map(str::to_string),
        identifier: account["account_identifier"].as_str().filter(|i| identifier_ok(i)).map(str::to_string),
        label: account["account_display_name"].as_str().filter(|l| held && label_ok(l)).map(str::to_string),
        state,
        scopes: held.then(|| pipes_scopes(account)).flatten(),
        created_at: account["created_at"].as_str().filter(|t| t.len() <= 64).unwrap_or_default().to_string(),
        fenced: false,
    }
}

/// A person's accounts at `provider`, as Pipes' plural list answers them
/// (`GET /user_management/users/{user}/data_providers?
/// supports_multiple_connections=true`: `data`, one entry per provider by
/// its `slug`, each with every one of its `connected_accounts`): those
/// connected or needing authorizing again, oldest first (by `created_at`,
/// then id), so the default is stable (`default_of`). No entry is none.
/// An account with no id, or more than `catalog::ACCOUNTS_MAX`, is a
/// fault: nothing of such an answer is used.
pub fn listed(answer: &Value, provider: &str) -> Result<Vec<PipesAccount>, String> {
    let entries = answer["data"].as_array().ok_or("Pipes' list of providers has no data")?;
    let Some(entry) = entries.iter().find(|e| e["slug"] == provider) else { return Ok(vec![]) };
    let accounts = match &entry["connected_accounts"] {
        Value::Null => return Ok(vec![]),
        Value::Array(a) => a,
        _ => return Err(format!("Pipes' {provider} entry lists its accounts as no list")),
    };
    if accounts.len() > usize::from(crate::catalog::ACCOUNTS_MAX) {
        return Err(format!("Pipes lists {} accounts at {provider}, more than its limit of {}", accounts.len(), crate::catalog::ACCOUNTS_MAX));
    }
    let mut out = Vec::with_capacity(accounts.len());
    for a in accounts {
        let account = described(a);
        if account.id.is_none() {
            return Err(format!("Pipes lists an account at {provider} with no id"));
        }
        if account.state != ProviderState::NotConnected {
            out.push(account);
        }
    }
    out.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
    Ok(out)
}

/// The account a person's agents use at a provider unless one picked
/// another: the oldest connected one, else (none connected) the oldest;
/// never one they disconnected.
pub fn default_of(accounts: &[PipesAccount]) -> Option<&PipesAccount> {
    let theirs = || accounts.iter().filter(|a| !a.fenced);
    theirs().find(|a| a.state == ProviderState::Connected).or(theirs().next())
}

/// A provider's state, from its accounts: connected while any is.
pub fn state_of(accounts: &[PipesAccount]) -> ProviderState {
    match default_of(accounts) {
        Some(a) => a.state,
        None => ProviderState::NotConnected,
    }
}

/// A person's accounts, each a disconnect's lasting fence stands for marked
/// `fenced` and not connected: all of them while the provider is fenced
/// whole, else each whose identifier is fenced (one that names none is
/// never fenced so: `lasting`). Pipes holds them still (a disconnect that
/// did not finish, or an import that landed after it), and settings offers
/// to disconnect them again; no agent uses one.
pub fn fenced(accounts: Vec<PipesAccount>, whole: bool, identifiers: &[&str]) -> Vec<PipesAccount> {
    accounts
        .into_iter()
        .map(|a| match whole || a.identifier.as_deref().is_some_and(|i| identifiers.contains(&i)) {
            true => PipesAccount { state: ProviderState::NotConnected, fenced: true, ..a },
            false => a,
        })
        .collect()
}

/// What of a disconnect stays fenced until a connect is known to import
/// again (registry/connection_signin.rs `fence_connection`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lasting {
    /// The provider whole: nothing Pipes holds there is taken for the
    /// person's (as with one account).
    Provider,
    /// The account of this identifier alone (`connection_signin::FENCE_ACCOUNT`).
    Account(String),
    /// Nothing: an account that names no identifier, which no later import
    /// can be told apart from the person's others by.
    Nothing,
}

/// What a disconnect of `gone`, an account of a person's at a provider
/// that takes several, leaves fenced, with `others` of theirs there still
/// (not fenced): that account, by its identifier; one that names none
/// (imported before identifiers were, or with no ID token), nothing while
/// they have others, whom a fence of it could not tell from it; the last
/// of theirs, or none, the provider whole, as with one.
pub fn lasting(gone: Option<&PipesAccount>, others: usize) -> Lasting {
    match gone.map(|a| a.identifier.clone()) {
        Some(Some(identifier)) => Lasting::Account(identifier),
        Some(None) if others > 0 => Lasting::Nothing,
        _ => Lasting::Provider,
    }
}

/// An account as a computer holds it, to choose one for a swap: its id and
/// state, in its person's list's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub id: String,
    pub state: ProviderState,
}

/// What a computer holds of each account: those with an id, in order,
/// never one the person disconnected.
pub fn held(accounts: &[PipesAccount]) -> Vec<Held> {
    accounts.iter().filter(|a| !a.fenced).filter_map(|a| a.id.clone().map(|id| Held { id, state: a.state })).collect()
}

/// Why no account is chosen for a swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unchosen {
    /// The account the agent uses must be authorized again: its own pick,
    /// never another account in its place, or the default.
    NeedsReauthorization,
    /// No account is connected.
    NotConnected,
}

/// The account an agent's swap uses: its pick while that account is
/// listed (connected; needing authorizing again, refused, never another
/// in its place), else the default (`default_of`): a pick whose account is
/// gone falls back to it.
pub fn chosen<'a>(pick: Option<&str>, accounts: &'a [Held]) -> Result<&'a str, Unchosen> {
    let one = pick.and_then(|p| accounts.iter().find(|a| a.id == p)).or_else(|| accounts.iter().find(|a| a.state == ProviderState::Connected)).or(accounts.first());
    match one {
        Some(a) if a.state == ProviderState::Connected => Ok(&a.id),
        Some(_) => Err(Unchosen::NeedsReauthorization),
        None => Err(Unchosen::NotConnected),
    }
}

/// What a connect is for, as `POST …/authorize` asks it (its body) and
/// its pending attempt keeps it: an account (a first, or another), or one
/// of the person's, by its id, connected again (`{account}`: Reconnect to
/// add, or its Connect again).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// No body: a connect, as with one account.
    Connect,
    /// `{add: true}`: another account.
    Add,
    /// `{account: "<id>"}`: that account, again.
    Reconnect(String),
}

impl Intent {
    /// The body of `POST …/authorize`: nothing (or `{}`), `{add: true}`,
    /// or `{account: "<id>"}`, never both.
    pub fn asked(body: &Value) -> Result<Intent, String> {
        let Some(o) = body.as_object() else { return if body.is_null() { Ok(Intent::Connect) } else { Err("a connect's body is an object".into()) } };
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "add" | "account")) {
            return Err(format!("a connect names `add` or `account`, not {k:?}"));
        }
        match (o.get("add"), o.get("account")) {
            (None, None) => Ok(Intent::Connect),
            (Some(Value::Bool(true)), None) => Ok(Intent::Add),
            (None, Some(Value::String(id))) if valid_account_id(id) => Ok(Intent::Reconnect(id.clone())),
            (None, Some(_)) => Err("an account is its id, as GET /api/connections lists it".into()),
            (Some(_), None) => Err("`add` is true, or absent".into()),
            (Some(_), Some(_)) => Err("a connect adds an account or connects one again, not both".into()),
        }
    }

    /// Its pending row's columns: `intent` and `account`.
    pub fn columns(&self) -> (&'static str, Option<&str>) {
        match self {
            Intent::Connect => ("connect", None),
            Intent::Add => ("add", None),
            Intent::Reconnect(id) => ("reconnect", Some(id)),
        }
    }

    /// Read back from its pending row's columns. None: no intent this code
    /// knows (a newer release's), which finishes nothing.
    pub fn of_columns(intent: &str, account: Option<&str>) -> Option<Intent> {
        match (intent, account) {
            ("connect", None) => Some(Intent::Connect),
            ("add", None) => Some(Intent::Add),
            ("reconnect", Some(id)) if valid_account_id(id) => Some(Intent::Reconnect(id.to_string())),
            _ => None,
        }
    }
}

/// The import a callback makes at Pipes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Import {
    /// The compatibility connection (no intent): a person's first account,
    /// as with one account (a 409 then updates it).
    Compatibility,
    /// Another account (`connection_intent: "add"`).
    Add,
    /// An account connected before, its grant replaced (`PUT`, naming it,
    /// `connection_intent: "reauthorize"`).
    Reauthorize(String),
}

/// A connect refused before Pipes is asked: the person has the row's
/// `accounts_max` accounts already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// The import a callback makes, from what the person asked (`intent`),
/// the accounts Pipes lists for them there (`listed`: every one, fenced
/// or not), and the grant's identifier (`connection_signin::
/// account_identifier`; None: no ID token). Pipes tells no duplicate
/// apart, so this does: a grant of an account already listed (its
/// identifier) reauthorizes that account, whatever was asked. A reconnect
/// of an account listed reauthorizes it unless the grant is plainly
/// another's (both name identifiers, and they differ), which is added. A
/// person's first account is the compatibility connection; another is
/// added, unless they have `max` already.
pub fn import_of(intent: &Intent, listed: &[PipesAccount], identifier: Option<&str>, max: u8) -> Result<Import, Full> {
    let id_of = |a: &PipesAccount| a.id.clone().expect("a listed account has an id");
    if let Some(same) = identifier.and_then(|i| listed.iter().find(|a| a.identifier.as_deref() == Some(i))) {
        return Ok(Import::Reauthorize(id_of(same)));
    }
    if let Intent::Reconnect(id) = intent {
        if let Some(again) = listed.iter().find(|a| a.id.as_deref() == Some(id)) {
            if again.identifier.is_none() || identifier.is_none() {
                return Ok(Import::Reauthorize(id_of(again)));
            }
        }
    }
    if listed.is_empty() {
        return Ok(Import::Compatibility);
    }
    if listed.len() >= usize::from(max) {
        return Err(Full);
    }
    Ok(Import::Add)
}

/// An import's body for several accounts: the account's identifier (so a
/// later grant of it is told apart), and an addition's intent.
pub fn import_body(mut body: Value, import: &Import, identifier: Option<&str>) -> Value {
    if let Some(i) = identifier {
        body["account_identifier"] = json!(i);
    }
    if *import == Import::Add {
        body["connection_intent"] = json!("add");
    }
    body
}

/// A reauthorization's query (the account it names, and its intent), as
/// WorkOS' SDK sends them on `PUT` (`updateUserConnectedAccount`).
pub fn reauthorize_query(id: &str) -> String {
    assert!(valid_account_id(id), "a listed account's id");
    url::form_urlencoded::Serializer::new(String::new()).append_pair("connected_account_id", id).append_pair("connection_intent", "reauthorize").finish()
}

/// What Pipes' answer to an import means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Imported {
    /// 200 or 201: the account is connected.
    Done,
    /// 409 of the compatibility connection: it is there, and is updated.
    Conflict,
    /// 409 `connected_account_limit_reached`: Pipes holds its most.
    Full,
    /// 404 `multiple_connections_unavailable`: Pipes connects one account
    /// a person for now (until WorkOS enables additional connections).
    Unavailable,
    /// Another 4xx: refused, nothing imported.
    Refused,
    /// Anything else: its outcome unknown (a 5xx, no answer), never asked
    /// again; it may yet land at Pipes.
    Failed,
}

/// `status` and its `answer` (Pipes' error `code`), read.
pub fn imported(status: u16, answer: &Value) -> Imported {
    let code = answer["code"].as_str().or(answer["error"].as_str());
    match (status, code) {
        (200 | 201, _) => Imported::Done,
        (404, Some("multiple_connections_unavailable")) => Imported::Unavailable,
        (409, Some("connected_account_limit_reached")) => Imported::Full,
        (409, _) => Imported::Conflict,
        (400..=499, _) => Imported::Refused,
        _ => Imported::Failed,
    }
}

/// A callback's longest wait for another's decision at the same account
/// (`Deciding`): its read of the list and its import, each within the
/// vendor's 30 s, and an update after a 409.
pub const DECIDE_WAIT_MS: i64 = 95_000;
/// How long an import whose outcome is unknown keeps others from deciding
/// at its account (`Deciding::release`): it may land meanwhile, and a list
/// read before it does would lack it.
pub const UNCERTAIN_HOLD_MS: i64 = 60_000;

/// The callbacks deciding which import to make at an account of several
/// (`connection_signin::Account`: a person's at a provider), one at a time
/// from its read of the list to its import's answer, so two that finish
/// together never both add the same provider account, nor both pass the
/// row's `accounts_max`: the second reads the list once the first has
/// imported. An import whose outcome is unknown holds the account a while
/// longer (`UNCERTAIN_HOLD_MS`), refusing others, which could read a list
/// that lacks it. The Registry keeps it in memory, as `Finishing`: a
/// restarted one has none under way. At most one hold a callback under
/// way, and the uncertain ones of the last `UNCERTAIN_HOLD_MS`.
#[derive(Debug, Default)]
pub struct Deciding {
    holds: Vec<Hold>,
}

#[derive(Debug)]
struct Hold {
    account: crate::connection_signin::Account,
    /// The callback deciding, or none: an import's outcome unknown, until
    /// `until`.
    by: Option<u64>,
    until: i64,
}

/// What a callback's ask to decide at an account finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// Its turn: it holds the account until it releases it.
    Taken,
    /// Another callback decides there now: ask again shortly.
    Busy,
    /// An import there has no known outcome yet: refused.
    Uncertain,
}

impl Deciding {
    /// Callback `id` asks to decide at `account`, at `now`.
    pub fn take(&mut self, account: &crate::connection_signin::Account, id: u64, now: i64) -> Turn {
        self.holds.retain(|h| h.by.is_some() || h.until > now);
        assert!(!self.holds.iter().any(|h| h.by == Some(id)), "a callback decides once");
        match self.holds.iter().find(|h| h.account == *account) {
            Some(h) if h.by.is_some() => Turn::Busy,
            Some(_) => Turn::Uncertain,
            None => {
                self.holds.push(Hold { account: account.clone(), by: Some(id), until: i64::MAX });
                Turn::Taken
            }
        }
    }

    /// Every import whose outcome was unknown is let go, as its while
    /// running out (a test's lever).
    pub fn forget_uncertain(&mut self) {
        self.holds.retain(|h| h.by.is_some());
    }

    /// Callback `id`'s turn over: its import's outcome known (None), or
    /// unknown, holding the account until `uncertain_until`.
    pub fn release(&mut self, id: u64, uncertain_until: Option<i64>) {
        let at = self.holds.iter().position(|h| h.by == Some(id)).expect("a callback releases the turn it took");
        match uncertain_until {
            Some(until) => {
                self.holds[at].by = None;
                self.holds[at].until = until;
            }
            None => {
                self.holds.remove(at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, identifier: Option<&str>, state: &str, created_at: &str) -> Value {
        json!({ "object": "connected_account", "id": id, "account_identifier": identifier, "account_display_name": format!("{id}@example.com"), "state": state, "scopes": ["mail.read"], "created_at": created_at, "connection_role": "standard" })
    }

    fn list(provider: &str, accounts: Vec<Value>) -> Value {
        json!({ "object": "list", "data": [{ "object": "data_provider", "slug": "other", "connected_accounts": [account("data_installation_other", None, "connected", "2026-01-01T00:00:00.000Z")] }, { "object": "data_provider", "slug": provider, "connected_account": null, "connected_accounts": accounts }] })
    }

    fn ids(accounts: &[PipesAccount]) -> Vec<&str> {
        accounts.iter().map(|a| a.id.as_deref().unwrap()).collect()
    }

    #[test]
    fn a_persons_accounts_are_listed_oldest_first_and_the_default_is_the_oldest_connected() {
        let answer = list("google", vec![
            account("data_installation_c", Some("sub-c"), "connected", "2026-10-03T00:00:00.000Z"),
            account("data_installation_b", Some("sub-b"), "needs_reauthorization", "2026-10-01T00:00:00.000Z"),
            account("data_installation_a", Some("sub-a"), "connected", "2026-10-02T00:00:00.000Z"),
            account("data_installation_a0", None, "connected", "2026-10-02T00:00:00.000Z"),
            account("data_installation_gone", Some("sub-x"), "disconnected", "2026-09-01T00:00:00.000Z"),
        ]);
        let accounts = listed(&answer, "google").unwrap();
        assert_eq!(ids(&accounts), ["data_installation_b", "data_installation_a", "data_installation_a0", "data_installation_c"], "by when each was made, then id; a state Pipes knows that is neither is left out");
        assert_eq!(default_of(&accounts).and_then(|a| a.id.as_deref()), Some("data_installation_a"), "the oldest connected, not the oldest");
        assert_eq!(state_of(&accounts), ProviderState::Connected);
        assert_eq!(accounts[1].identifier.as_deref(), Some("sub-a"));
        assert_eq!(accounts[1].label.as_deref(), Some("data_installation_a@example.com"));
        assert_eq!(accounts[1].scopes, Some(vec!["mail.read".to_string()]));
        let waiting = &accounts[..1];
        assert_eq!((default_of(waiting).and_then(|a| a.id.as_deref()), state_of(waiting)), (Some("data_installation_b"), ProviderState::NeedsReauthorization), "none connected: the oldest");
        assert_eq!((default_of(&[]), state_of(&[])), (None, ProviderState::NotConnected));
        assert_eq!(listed(&list("google", vec![]), "google").unwrap(), vec![]);
        assert_eq!(listed(&json!({ "data": [] }), "google").unwrap(), vec![], "no entry is no account");
        assert_eq!(listed(&json!({ "data": [{ "slug": "google", "connected_accounts": null }] }), "google").unwrap(), vec![]);
        for bad in [
            json!({}),
            json!({ "data": [{ "slug": "google", "connected_accounts": "x" }] }),
            list("google", vec![account("not an id", None, "connected", "2026-10-01T00:00:00.000Z")]),
            list("google", (0..26).map(|i| account(&format!("data_installation_{i}"), None, "connected", "2026-10-01T00:00:00.000Z")).collect()),
        ] {
            assert!(listed(&bad, "google").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_fenced_identifier_marks_its_accounts_and_no_other() {
        let accounts = listed(&list("google", vec![account("data_installation_a", Some("sub-a"), "connected", "1"), account("data_installation_b", Some("sub-b"), "connected", "2"), account("data_installation_n", None, "connected", "3")]), "google").unwrap();
        let marked = |l: Vec<PipesAccount>| l.iter().map(|a| (a.fenced, a.state)).collect::<Vec<_>>();
        use ProviderState::{Connected, NotConnected};
        let one = fenced(accounts.clone(), false, &["sub-a"]);
        assert_eq!(marked(one.clone()), [(true, NotConnected), (false, Connected), (false, Connected)], "its identifier's account is marked, not dropped: Pipes holds it");
        assert_eq!(default_of(&one).and_then(|a| a.id.as_deref()), Some("data_installation_b"), "never the default");
        assert_eq!(held(&one).iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["data_installation_b", "data_installation_n"], "nor held for a swap");
        assert_eq!(marked(fenced(accounts.clone(), false, &["", "sub-x"])), [(false, Connected); 3], "an account that names no identifier is never fenced by one");
        assert_eq!(marked(fenced(accounts.clone(), true, &[])), [(true, NotConnected); 3], "the provider whole: each");
        assert_eq!(state_of(&fenced(accounts.clone(), true, &[])), NotConnected);
        assert_eq!(default_of(&fenced(accounts.clone(), true, &[])), None);
        // what a disconnect leaves fenced
        let (a, n) = (&accounts[0], &accounts[2]);
        let of_a = Lasting::Account("sub-a".into());
        assert_eq!(lasting(Some(a), 2), of_a, "an account, by its identifier");
        assert_eq!(lasting(Some(a), 0), of_a, "the last too");
        assert_eq!(lasting(Some(n), 2), Lasting::Nothing, "one with none, while others are there, which a fence of it would hide");
        assert_eq!(lasting(Some(n), 0), Lasting::Provider, "the last, with none: the provider whole, as with one");
        assert_eq!(lasting(None, 0), Lasting::Provider, "none there: the provider whole");
    }

    fn at(subject: &str) -> crate::connection_signin::Account {
        crate::connection_signin::Account { issuer: "issuer".into(), subject: subject.into(), provider: "google".into() }
    }

    #[test]
    fn one_callback_at_a_time_decides_at_an_account_and_an_unknown_import_holds_it() {
        let mut d = Deciding::default();
        let (a, b) = (at("user-a"), at("user-b"));
        assert_eq!(d.take(&a, 1, 0), Turn::Taken);
        assert_eq!(d.take(&a, 2, 0), Turn::Busy, "a second callback of the same person and provider waits");
        assert_eq!(d.take(&b, 3, 0), Turn::Taken, "another person's does not");
        d.release(1, None);
        assert_eq!(d.take(&a, 2, 10), Turn::Taken, "the first answered: the second's turn, reading the list afresh");
        d.release(2, Some(100));
        assert_eq!(d.take(&a, 4, 50), Turn::Uncertain, "an import whose outcome is unknown refuses others while it may land");
        assert_eq!(d.take(&a, 5, 100), Turn::Taken, "and holds no longer than its while");
        d.release(5, Some(1_000));
        d.forget_uncertain();
        assert_eq!(d.take(&a, 6, 200), Turn::Taken, "forgotten, it holds nothing");
        d.release(6, None);
        d.release(3, None);
        assert!(d.holds.is_empty());
    }

    #[test]
    #[should_panic(expected = "a callback releases the turn it took")]
    fn a_turn_is_released_once() {
        let mut d = Deciding::default();
        assert_eq!(d.take(&at("user-a"), 1, 0), Turn::Taken);
        d.release(1, None);
        d.release(1, None);
    }

    #[test]
    fn a_swap_uses_its_agents_pick_while_it_is_listed_else_the_default() {
        let held = |l: &[(&str, ProviderState)]| l.iter().map(|(id, state)| Held { id: id.to_string(), state: *state }).collect::<Vec<_>>();
        use ProviderState::{Connected, NeedsReauthorization};
        let two = held(&[("old", NeedsReauthorization), ("a", Connected), ("b", Connected)]);
        assert_eq!(chosen(None, &two), Ok("a"), "the default: the oldest connected");
        assert_eq!(chosen(Some("b"), &two), Ok("b"), "its pick");
        assert_eq!(chosen(Some("gone"), &two), Ok("a"), "a pick whose account is gone: the default");
        assert_eq!(chosen(Some("old"), &two), Err(Unchosen::NeedsReauthorization), "a pick that needs authorizing again is refused, never swapped for another account");
        assert_eq!(chosen(None, &held(&[("old", NeedsReauthorization)])), Err(Unchosen::NeedsReauthorization));
        assert_eq!(chosen(Some("b"), &[]), Err(Unchosen::NotConnected));
    }

    #[test]
    fn a_connect_asks_for_an_account_another_or_one_again_and_its_row_keeps_it() {
        assert_eq!(Intent::asked(&Value::Null), Ok(Intent::Connect));
        assert_eq!(Intent::asked(&json!({})), Ok(Intent::Connect));
        assert_eq!(Intent::asked(&json!({ "add": true })), Ok(Intent::Add));
        assert_eq!(Intent::asked(&json!({ "account": "data_installation_1" })), Ok(Intent::Reconnect("data_installation_1".into())));
        for bad in [json!({ "add": false }), json!({ "add": "yes" }), json!({ "account": "a b" }), json!({ "account": 7 }), json!({ "add": true, "account": "data_installation_1" }), json!({ "other": 1 }), json!([])] {
            assert!(Intent::asked(&bad).is_err(), "{bad}");
        }
        for intent in [Intent::Connect, Intent::Add, Intent::Reconnect("data_installation_1".into())] {
            let (kind, account) = intent.columns();
            assert_eq!(Intent::of_columns(kind, account), Some(intent.clone()), "{intent:?} reads back from its row");
        }
        assert_eq!(Intent::of_columns("connect", Some("x")), None);
        assert_eq!(Intent::of_columns("reconnect", None), None);
        assert_eq!(Intent::of_columns("merge", None), None, "a newer release's intent finishes nothing");
    }

    #[test]
    fn a_grant_of_an_account_already_connected_reauthorizes_it_and_never_adds_a_second() {
        let accounts = listed(&list("google", vec![account("data_installation_a", Some("sub-a"), "connected", "1"), account("data_installation_n", None, "needs_reauthorization", "2")]), "google").unwrap();
        let reauthorize = |id: &str| Ok(Import::Reauthorize(id.into()));
        let again = |id: &str| Intent::Reconnect(id.into());
        for intent in [Intent::Connect, Intent::Add, again("data_installation_n")] {
            assert_eq!(import_of(&intent, &accounts, Some("sub-a"), 3), reauthorize("data_installation_a"), "the same identifier, whatever was asked: {intent:?}");
        }
        assert_eq!(import_of(&Intent::Add, &accounts, Some("sub-new"), 3), Ok(Import::Add), "another account");
        assert_eq!(import_of(&Intent::Connect, &accounts, None, 3), Ok(Import::Add), "no identifier is told apart from none");
        assert_eq!(import_of(&again("data_installation_n"), &accounts, Some("sub-new"), 3), reauthorize("data_installation_n"), "a reconnect of an account that names no identifier");
        assert_eq!(import_of(&again("data_installation_a"), &accounts, None, 3), reauthorize("data_installation_a"), "a reconnect whose grant names none");
        assert_eq!(import_of(&again("data_installation_a"), &accounts, Some("sub-other"), 3), Ok(Import::Add), "a reconnect plainly of another account adds it, never replacing the one pressed");
        assert_eq!(import_of(&again("data_installation_gone"), &accounts, Some("sub-new"), 3), Ok(Import::Add), "a reconnect of an account gone since");
        assert_eq!(import_of(&Intent::Add, &accounts, Some("sub-new"), 2), Err(Full), "at the row's most");
        assert_eq!(import_of(&Intent::Add, &accounts, Some("sub-a"), 2), reauthorize("data_installation_a"), "at its most, an account already connected is still reauthorized");
        for intent in [Intent::Connect, Intent::Add, again("data_installation_a")] {
            assert_eq!(import_of(&intent, &[], Some("sub-a"), 3), Ok(Import::Compatibility), "a first account is the compatibility connection: {intent:?}");
        }
    }

    #[test]
    fn an_imports_body_query_and_answer() {
        let body = json!({ "access_token": "a", "state": "connected" });
        assert_eq!(import_body(body.clone(), &Import::Add, Some("sub-a")), json!({ "access_token": "a", "state": "connected", "account_identifier": "sub-a", "connection_intent": "add" }));
        assert_eq!(import_body(body.clone(), &Import::Compatibility, Some("sub-a")), json!({ "access_token": "a", "state": "connected", "account_identifier": "sub-a" }), "a first account has no intent");
        assert_eq!(import_body(body.clone(), &Import::Reauthorize("data_installation_a".into()), None), body, "a reauthorization's intent is its query's");
        assert_eq!(reauthorize_query("data_installation_a"), "connected_account_id=data_installation_a&connection_intent=reauthorize");
        assert_eq!(imported(201, &json!({})), Imported::Done);
        assert_eq!(imported(200, &Value::Null), Imported::Done);
        assert_eq!(imported(404, &json!({ "code": "multiple_connections_unavailable", "message": "…" })), Imported::Unavailable);
        assert_eq!(imported(409, &json!({ "code": "connected_account_limit_reached" })), Imported::Full);
        assert_eq!(imported(409, &json!({ "code": "conflict" })), Imported::Conflict);
        assert_eq!(imported(409, &Value::Null), Imported::Conflict);
        for (status, answer) in [(404, json!({ "code": "not_found" })), (422, json!({ "code": "invalid_tokens" })), (400, Value::Null)] {
            assert_eq!(imported(status, &answer), Imported::Refused, "{status} {answer}");
        }
        for (status, answer) in [(504, Value::Null), (500, json!({ "code": "multiple_connections_unavailable" })), (0, Value::Null), (302, Value::Null)] {
            assert_eq!(imported(status, &answer), Imported::Failed, "{status} {answer}: its outcome unknown");
        }
    }
}
