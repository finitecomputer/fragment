//! A wipe's `connections` step (FIN-588; `wipe::Step::Connections`, run by
//! cell/src/connections.rs `wipe`): each account WorkOS Pipes holds for
//! the wiped person's WorkOS user, at each connection of the catalog, its
//! grant revoked at its provider and only then its account deleted at
//! Pipes, so neither the provider's grant nor Pipes' tokens outlive them,
//! and the new person their next sign-in makes (the same WorkOS user: a
//! wipe leaves it) inherits none of them. This file decides what the cell
//! would otherwise decide on its own:
//!
//! - **which accounts** (`listed`): each the plural list names, by its id
//!   alone, so a call that stalled past the wipe's end never takes the
//!   next person's account (an account-less call would reach whatever
//!   account is there then);
//! - **when the step may act** (`may_end`): only while the wipe stands
//!   before this step, asked before Pipes is listed and again after, so a
//!   list is only acted on when it was made while the person was still
//!   being wiped, before anyone new could connect;
//! - **what a try did** (`vended`, `revoked`) and **what follows**
//!   (`then`): an account is deleted once its grant is revoked or known to
//!   be gone, which only a well-formed answer saying so shows; a revoke
//!   that failed keeps the account (its token is what a retry revokes
//!   with) and fails the step, until it has failed `REVOKE_TRIES_MAX`
//!   calls over at least `REVOKE_GIVE_UP_AFTER_MS`, when the step gives
//!   that account up: deleted at Pipes, said in the report and the log,
//!   the provider's grant perhaps left. A call whose time ran out before a
//!   vendor call is no failure (`Tried::Cut`);
//! - **one account's end, in order** (`end_account`, over the cell's
//!   `Vendor`): its vend, its revoke, its end recorded (`Kept::ended`, in
//!   `wipe_revokes`) and only then its delete, so a call cut after the
//!   revoke, or a delete that failed, resumes at the delete, never at a
//!   revoke the provider would now refuse; and **what the step says**
//!   (`Wiped`): an account given up is said deleted at Pipes only once its
//!   delete answered.

use serde_json::Value;

use crate::connection_accounts::{entry_accounts, valid_account_id};
use crate::wipe::{Progress, Step};

/// Calls in which an account's revoke may fail before the step gives it up.
pub const REVOKE_TRIES_MAX: u32 = 5;
/// How long after an account's first failed revoke the step may give it
/// up: a provider's outage of a few hours never costs a revoke, whatever
/// the CLI's pace (`WIPE_PAUSE`, two seconds between calls).
pub const REVOKE_GIVE_UP_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

/// The step acts only while the wipe stands before it: a call that read
/// the person's WorkOS user, then stalled while another finished the wipe
/// (the registry forgets them, and their next sign-in is someone new under
/// the same WorkOS user), finds it past and changes nothing.
pub fn may_end(progress: Option<Progress>) -> Result<(), String> {
    match progress {
        None => Err("no wipe of theirs began".into()),
        Some(p) if p.next() == Some(Step::Connections) => Ok(()),
        Some(p) => Err(format!("their wipe stands at {}, not connections: nothing of theirs is ended here", p.next().map_or("its end", Step::name))),
    }
}

/// The ids of the accounts Pipes' plural list (`GET /user_management/
/// users/{user}/data_providers?supports_multiple_connections=true`) names
/// at `provider`: each of `connected_accounts`, in any state (one needing
/// authorizing again, or one Pipes calls disconnected, may still hold what
/// Pipes keeps of its grant); where an entry names none so, its one
/// `connected_account`. An account with no valid id is a fault: no account
/// is reached by none (`listed`'s docs).
pub fn listed(answer: &Value, provider: &str) -> Result<Vec<String>, String> {
    let mut accounts: Vec<&Value> = entry_accounts(answer, provider)?.iter().collect();
    if accounts.is_empty() {
        let entry = answer["data"].as_array().and_then(|d| d.iter().find(|e| e["slug"] == provider));
        if let Some(one) = entry.map(|e| &e["connected_account"]).filter(|a| a.is_object()) {
            accounts.push(one);
        }
    }
    let ids: Vec<String> = accounts
        .iter()
        .map(|a| a["id"].as_str().filter(|i| valid_account_id(i)).map(str::to_string).ok_or_else(|| format!("Pipes lists an account at {provider} with no id")))
        .collect::<Result<_, _>>()?;
    assert!(ids.len() <= usize::from(crate::catalog::ACCOUNTS_MAX), "a provider's accounts are bounded");
    Ok(ids)
}

/// A token Pipes vended for an account's revoke, as Pipes answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vended {
    /// An access token, which revokes the grant behind it.
    Token(String),
    /// None, as Pipes holds no grant usable for one: the account gone (404),
    /// or a well-formed answer that it is inactive (`{active: false}`, with
    /// no error or one Pipes names: its provider refused its refresh,
    /// `needs_reauthorization`; or none installed, `not_installed`). The
    /// provider's grant is gone, or out of reach of anything Pipes holds.
    NoGrant,
    /// Pipes did not answer, answered a fault, or answered what it never
    /// says (a 200 cut short or unreadable is JSON null here): the grant may live.
    Failed(String),
}

/// What Pipes says of an inactive account (WorkOS' API reference, the
/// token's `error`): any other is no proof of anything.
pub const INACTIVE_ERRORS: [&str; 2] = ["needs_reauthorization", "not_installed"];

/// Pipes' vend (`POST /data-integrations/{provider}/token`) as a revoke reads it.
pub fn vended(status: u16, answer: &Value) -> Vended {
    let inactive = answer["active"] == false && answer.get("error").is_none_or(|e| e.as_str().is_some_and(|e| INACTIVE_ERRORS.contains(&e)));
    match status {
        200 if answer["active"] == true => match answer["access_token"]["access_token"].as_str().filter(|t| !t.is_empty()) {
            Some(t) => Vended::Token(t.to_string()),
            None => Vended::Failed("Pipes vended an active token with none in it".into()),
        },
        200 if inactive => Vended::NoGrant,
        200 => Vended::Failed("Pipes answered its vend with neither a token nor that it holds none".into()),
        404 => Vended::NoGrant,
        s => Vended::Failed(format!("Pipes did not vend a token to revoke ({s})")),
    }
}

/// One account's revoke, tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tried {
    /// The provider confirmed it.
    Revoked,
    /// Nothing left to revoke: Pipes holds no usable grant, or the row
    /// names no revoke.
    NoGrant,
    /// The grant may live: kept, and tried again.
    Failed(String),
    /// The call's time ran out before its vend: not tried, no failure.
    Cut,
}

/// A provider's revocation answer (RFC 7009 §2.2): 200 revoked. Anything
/// else may leave the grant, Google's 400 `invalid_token` too: the token
/// revoked is the access token Pipes vended (Pipes vends no refresh
/// token), and one invalid (expired, or revoked alone) says nothing of the
/// refresh token behind it. A grant ended at Google already is shown once
/// Pipes' refresh is refused (`Vended::NoGrant`), or given up.
pub fn revoked(status: u16, _answer: &Value) -> Tried {
    match status {
        200 => Tried::Revoked,
        s => Tried::Failed(format!("the provider did not revoke it ({s})")),
    }
}

/// What the step does with an account after a try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Delete it at Pipes: its grant is revoked, or gone.
    Delete,
    /// Keep it, its tries counted: the step fails this call, and the next
    /// tries again with the token Pipes still holds.
    Retry,
    /// Give it up: deleted at Pipes though its revoke never succeeded,
    /// after `REVOKE_TRIES_MAX` failed calls over `REVOKE_GIVE_UP_AFTER_MS`.
    /// Said so in the report and the log: the provider may keep its grant
    /// (no token of it is held anywhere of Finite's after).
    GiveUp,
    /// The call's time ran out: the next call tries it, nothing counted.
    Cut,
}

/// The tries an account's revoke has failed in earlier calls, and when the
/// first did (`None`: none failed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Failures {
    pub tries: u32,
    pub first_ms: Option<i64>,
}

/// What follows `tried` for an account whose earlier calls failed `before`, now `now_ms`.
pub fn then(tried: &Tried, before: Failures, now_ms: i64) -> Then {
    match tried {
        Tried::Revoked | Tried::NoGrant => Then::Delete,
        Tried::Cut => Then::Cut,
        Tried::Failed(_) => {
            let tries = before.tries.saturating_add(1);
            let since = before.first_ms.map_or(0, |f| now_ms.saturating_sub(f));
            match tries >= REVOKE_TRIES_MAX && since >= REVOKE_GIVE_UP_AFTER_MS {
                true => Then::GiveUp,
                false => Then::Retry,
            }
        }
    }
}

/// How an account's revoke ended, kept until its delete answered
/// (`wipe_revokes.ended`), so the next call deletes it at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Revoked, or known gone.
    Revoked,
    /// Given up (`Then::GiveUp`).
    GivenUp,
}

impl Ended {
    /// Its stored code: 0 none (its revoke still to try), 1, 2.
    pub fn code(ended: Option<Ended>) -> i64 {
        match ended {
            None => 0,
            Some(Ended::Revoked) => 1,
            Some(Ended::GivenUp) => 2,
        }
    }

    /// A stored code read back; any other is a fault.
    pub fn of_code(code: i64) -> Result<Option<Ended>, String> {
        match code {
            0 => Ok(None),
            1 => Ok(Some(Ended::Revoked)),
            2 => Ok(Some(Ended::GivenUp)),
            c => Err(format!("a wipe's revoke ended as {c}, which no release writes")),
        }
    }
}

/// What earlier calls kept of an account (`wipe_revokes`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kept {
    pub failures: Failures,
    pub ended: Option<Ended>,
}

/// What the step keeps of an account at the registry (`Vendor::note`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// Its revoke failed this call: counted.
    Failed,
    /// Its revoke ended so: kept until it is deleted.
    Ended(Ended),
}

/// The calls one account's end makes (the cell's: Pipes, the provider and
/// the registry; a test's fake). `vend` and `revoke` answer (status, the
/// answer as JSON, null when it is not) or why there was none, which fails
/// the try; `delete` and `note` failing fail the step's call.
// one implementer in the single-threaded Worker, one fake: no `Send` asked
#[allow(async_fn_in_trait)]
pub trait Vendor {
    type Error;
    /// Now, in ms since the epoch.
    fn now(&self) -> i64;
    async fn vend(&mut self, provider: &str, id: &str) -> Result<(u16, Value), String>;
    async fn revoke(&mut self, provider: &str, token: &str) -> Result<(u16, Value), String>;
    /// The account deleted at Pipes (gone already counts).
    async fn delete(&mut self, provider: &str, id: &str) -> Result<(), Self::Error>;
    /// `note` kept of the account; answers its failed tries so far.
    async fn note(&mut self, provider: &str, id: &str, note: Note) -> Result<u32, Self::Error>;
}

/// How one call left an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// Deleted at Pipes; `given_up` why, when its revoke never succeeded.
    Deleted { given_up: Option<String> },
    /// Kept, its revoke failed (`tries` calls so far): tried again.
    Kept { why: String, tries: u32 },
    /// The call's time ran out first: the next call goes on with it.
    Cut,
}

/// One account's end (`id` at `provider`, of which earlier calls `kept`
/// what they did): unless an earlier call ended its revoke, vended a
/// token (begun only before `deadline`) and revoked with it (`revokes`:
/// the row names a revoke; else there is nothing to revoke), at once, so
/// a token vended is never left unspent by a slow vend; then that end
/// recorded before the delete, which begins only before `deadline`. At
/// most two vendor calls run past the deadline, each within its timeout.
pub async fn end_account<V: Vendor>(v: &mut V, provider: &str, id: &str, revokes: bool, kept: Kept, deadline: i64) -> Result<Ending, V::Error> {
    let given_up = match kept.ended {
        Some(Ended::Revoked) => None,
        Some(Ended::GivenUp) => Some(format!("given up in an earlier call after {REVOKE_TRIES_MAX} failed revokes")),
        None => {
            let tried = match revokes {
                true => try_revoke(v, provider, id, deadline).await,
                false => Tried::NoGrant,
            };
            let (ended, why) = match (then(&tried, kept.failures, v.now()), tried) {
                (Then::Cut, _) => return Ok(Ending::Cut),
                (Then::Retry, Tried::Failed(why)) => {
                    let tries = v.note(provider, id, Note::Failed).await?;
                    return Ok(Ending::Kept { why, tries });
                }
                (Then::GiveUp, Tried::Failed(why)) => (Ended::GivenUp, Some(why)),
                (Then::Delete, _) => (Ended::Revoked, None),
                (t, tried) => unreachable!("{t:?} follows no {tried:?}"),
            };
            v.note(provider, id, Note::Ended(ended)).await?;
            why
        }
    };
    if v.now() > deadline {
        return Ok(Ending::Cut);
    }
    v.delete(provider, id).await?;
    Ok(Ending::Deleted { given_up })
}

/// `end_account`'s revoke: a vend begun before `deadline`, and the revoke with what it vended.
async fn try_revoke<V: Vendor>(v: &mut V, provider: &str, id: &str, deadline: i64) -> Tried {
    if v.now() > deadline {
        return Tried::Cut;
    }
    let token = match v.vend(provider, id).await {
        Ok((status, answer)) => match vended(status, &answer) {
            Vended::Token(t) => t,
            Vended::NoGrant => return Tried::NoGrant,
            Vended::Failed(why) => return Tried::Failed(why),
        },
        Err(why) => return Tried::Failed(why),
    };
    match v.revoke(provider, &token).await {
        Ok((status, answer)) => revoked(status, &answer),
        Err(why) => Tried::Failed(why),
    }
}

/// What a call of the step did, as its report says it.
#[derive(Debug, Default)]
pub struct Wiped {
    /// Accounts deleted at Pipes.
    pub deleted: u64,
    /// Accounts kept, their revoke failed this call (`provider id: why`).
    pub kept: Vec<String>,
    /// Accounts deleted at Pipes this call though their revoke never
    /// succeeded (`provider id: why`).
    pub given_up: Vec<String>,
    /// The call's time ran out before every account was ended.
    pub cut: bool,
}

impl Wiped {
    /// `ending` of the account `id` at `provider`, counted.
    pub fn add(&mut self, provider: &str, id: &str, ending: Ending) {
        match ending {
            Ending::Deleted { given_up } => {
                self.deleted += 1;
                if let Some(why) = given_up {
                    self.given_up.push(format!("{provider} {id}: {why}"));
                }
            }
            Ending::Kept { why, tries } => self.kept.push(format!("{provider} {id}: {why} (failed {tries} of at least {REVOKE_TRIES_MAX} calls)")),
            Ending::Cut => self.cut = true,
        }
    }

    /// The step is done: every account listed is deleted.
    pub fn done(&self) -> bool {
        self.kept.is_empty() && !self.cut
    }

    /// The step's note, when there is something to say.
    pub fn note(&self) -> Option<String> {
        let mut said = vec![];
        if !self.given_up.is_empty() {
            said.push(format!("given up after {REVOKE_TRIES_MAX} failed revokes over a day, deleted at Pipes, their grant perhaps left at the provider: {}", self.given_up.join("; ")));
        }
        if !self.kept.is_empty() {
            said.push(format!("not revoked, kept for the next call: {}", self.kept.join("; ")));
        }
        if self.cut {
            said.push("the call's time ran out: the next call ends the rest".into());
        }
        (!said.is_empty()).then(|| said.join(". "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use serde_json::json;

    fn account(id: &str, state: &str) -> Value {
        json!({ "object": "connected_account", "id": id, "state": state, "created_at": "2026-10-01T00:00:00.000Z" })
    }

    fn list(provider: &str, accounts: Value) -> Value {
        json!({ "object": "list", "data": [{ "slug": "other", "connected_accounts": [account("data_installation_other", "connected")] }, { "slug": provider, "connected_account": null, "connected_accounts": accounts }] })
    }

    /// Goal: the step ends each account the list names at a connection, by
    /// its id, in any state, of one account a person or several, and no
    /// other provider's; it never reaches an account by no id. Method: a
    /// list of three in three states; a single-account entry naming its
    /// one account only as `connected_account`; empty and faulty lists.
    #[test]
    fn the_step_ends_each_listed_account_by_its_id() {
        let three = list("google", json!([account("data_installation_b", "needs_reauthorization"), account("data_installation_a", "connected"), account("data_installation_x", "disconnected")]));
        assert_eq!(listed(&three, "google").unwrap(), ["data_installation_b", "data_installation_a", "data_installation_x"], "each, in any state, none of `other`'s");
        let compatibility = json!({ "data": [{ "slug": "notion", "connected_account": account("data_installation_c", "connected"), "connected_accounts": null }] });
        assert_eq!(listed(&compatibility, "notion").unwrap(), ["data_installation_c"], "one account a person: by its id too, never by none");
        assert!(listed(&json!({ "data": [] }), "google").unwrap().is_empty(), "none there: nothing to end");
        assert!(listed(&json!({ "data": [{ "slug": "google", "connected_account": null }] }), "google").unwrap().is_empty());
        let many: Vec<Value> = (0..26).map(|i| account(&format!("data_installation_{i}"), "connected")).collect();
        for bad in [json!({}), list("google", json!([account("not an id", "connected")])), list("google", json!([{ "state": "connected" }])), list("google", json!(many))] {
            assert!(listed(&bad, "google").is_err(), "{bad}");
        }
    }

    /// Goal: a call that stalled past the wipe's step (another finished the
    /// wipe, and someone new may have connected under the same WorkOS
    /// user) changes nothing, nor does one before the step; an old row
    /// that stopped before `registry` stands at it. Method: each count.
    #[test]
    fn the_step_acts_only_while_the_wipe_stands_before_it() {
        let at = Step::Connections.index() as i64;
        assert_eq!(may_end(Some(Progress::recorded(at, false).unwrap())), Ok(()));
        assert!(may_end(None).is_err(), "no wipe");
        for n in 0..at {
            assert!(may_end(Some(Progress::recorded(n, false).unwrap())).is_err(), "before it: {n}");
        }
        assert!(may_end(Some(Progress::recorded(at + 1, false).unwrap())).is_err(), "past it");
        assert!(may_end(Some(Progress::recorded(at + 2, true).unwrap())).unwrap_err().contains("its end"), "finished: the stale call");
    }

    /// Goal: a vend and a revoke read as what they say of the grant: a
    /// token to revoke, none left only when Pipes says so in an answer it
    /// gives, and anything else a fault that keeps the account: a 200 cut
    /// short or unreadable (null here), one of no shape Pipes answers, and
    /// the provider's `invalid_token` for the access token Pipes vended (the
    /// refresh token behind it may live). Method: each answer.
    #[test]
    fn a_try_reads_what_pipes_and_the_provider_said() {
        assert_eq!(vended(200, &json!({ "active": true, "access_token": { "access_token": "ya29.x" } })), Vended::Token("ya29.x".into()));
        assert_eq!(vended(200, &json!({ "active": false, "error": "needs_reauthorization" })), Vended::NoGrant);
        assert_eq!(vended(200, &json!({ "active": false, "error": "not_installed" })), Vended::NoGrant);
        assert_eq!(vended(200, &json!({ "active": false })), Vended::NoGrant, "none installed, as the token path says it");
        assert_eq!(vended(404, &Value::Null), Vended::NoGrant, "the account gone");
        assert!(matches!(vended(200, &json!({ "active": true, "access_token": { "access_token": "" } })), Vended::Failed(_)));
        for bad in [Value::Null, json!({}), json!([]), json!("ok"), json!({ "active": "false" }), json!({ "active": null }), json!({ "active": false, "error": "rate_limited" }), json!({ "active": false, "error": 7 }), json!({ "error": "needs_reauthorization" })] {
            assert!(matches!(vended(200, &bad), Vended::Failed(_)), "a 200 of no shape Pipes gives proves nothing: {bad}");
        }
        for s in [201, 401, 409, 429, 500, 503] {
            assert!(matches!(vended(s, &json!({ "active": false })), Vended::Failed(_)), "{s}");
        }
        assert_eq!(revoked(200, &Value::Null), Tried::Revoked);
        let invalid = json!({ "error": "invalid_token", "error_description": "Token expired or revoked" });
        assert!(matches!(revoked(400, &invalid), Tried::Failed(_)), "an access token invalid says nothing of its refresh token");
        for (s, a) in [(400, json!({ "error": "invalid_request" })), (401, Value::Null), (500, Value::Null), (503, json!({ "error": "invalid_token" }))] {
            assert!(matches!(revoked(s, &a), Tried::Failed(_)), "{s} {a}");
        }
    }

    /// One account at Pipes, a provider and the registry's `wipe_revokes`,
    /// each call taking its time on a clock the calls advance.
    struct Fake {
        now: i64,
        vend_ms: i64,
        vend: (u16, Value),
        revoke: (u16, Value),
        revokes: u32,
        vends: u32,
        delete_fails: bool,
        deleted: bool,
        kept: Kept,
    }

    impl Fake {
        fn new(vend_ms: i64, revoke: (u16, Value)) -> Self {
            let vend = (200, json!({ "active": true, "access_token": { "access_token": "ya29.x" } }));
            Fake { now: 1_000_000, vend_ms, vend, revoke, revokes: 0, vends: 0, delete_fails: false, deleted: false, kept: Kept::default() }
        }

        /// One call of the step: its 20 s budget, Pipes listed in 2 s, then the account's end.
        fn call(&mut self) -> Wiped {
            let deadline = self.now + 20_000;
            self.now += 2_000;
            let mut w = Wiped::default();
            if !self.deleted {
                let kept = self.kept;
                match end_account(self, "google", "data_installation_a", true, kept, deadline).now_or_never().expect("the fake answers at once") {
                    Ok(e) => w.add("google", "data_installation_a", e),
                    Err(()) => w.cut = true,
                }
            }
            w
        }
    }

    impl Vendor for Fake {
        type Error = ();
        fn now(&self) -> i64 {
            self.now
        }
        async fn vend(&mut self, _: &str, _: &str) -> Result<(u16, Value), String> {
            self.vends += 1;
            self.now += self.vend_ms;
            Ok(self.vend.clone())
        }
        async fn revoke(&mut self, _: &str, token: &str) -> Result<(u16, Value), String> {
            assert_eq!(token, "ya29.x", "the token Pipes vended");
            self.revokes += 1;
            self.now += 300;
            Ok(self.revoke.clone())
        }
        async fn delete(&mut self, _: &str, _: &str) -> Result<(), ()> {
            self.now += 300;
            assert!(self.kept.ended.is_some(), "an account's end is kept before its delete");
            if self.delete_fails {
                return Err(());
            }
            self.deleted = true;
            Ok(())
        }
        async fn note(&mut self, _: &str, _: &str, note: Note) -> Result<u32, ()> {
            match note {
                Note::Failed => {
                    self.kept.failures = Failures { tries: self.kept.failures.tries + 1, first_ms: self.kept.failures.first_ms.or(Some(self.now)) };
                }
                Note::Ended(e) => self.kept.ended = Some(e),
            }
            Ok(self.kept.failures.tries)
        }
    }

    /// Goal: a slow but working Pipes never stalls a wipe (Codex's case: a
    /// 2 s list and a 19 s vend in a 20 s budget): a token vended is spent
    /// on its revoke at once, the revoke kept, and the next call resumes at
    /// the delete, never revoking again (the provider would refuse a token
    /// it revoked) and nothing counted as a failure; a call whose time ran
    /// out before the vend counts nothing either. Method: calls of the step
    /// on the fake's clock.
    #[test]
    fn a_slow_vend_never_stalls_the_delete() {
        let mut f = Fake::new(19_000, (200, Value::Null));
        let first = f.call();
        assert!(!first.done() && first.cut, "the delete would begin past the deadline");
        assert_eq!((f.revokes, f.kept.ended, f.kept.failures.tries), (1, Some(Ended::Revoked), 0), "revoked, kept so, nothing failed");
        assert!(!first.note().unwrap().contains("deleted at Pipes"));
        let second = f.call();
        assert!(second.done() && f.deleted, "the next call deletes it");
        assert_eq!((f.vends, f.revokes, second.deleted), (1, 1, 1), "at once, no vend or revoke again");
        assert_eq!(second.note(), None);

        let mut f = Fake::new(19_000, (200, Value::Null));
        let deadline = f.now - 1;
        let cut = end_account(&mut f, "google", "data_installation_a", true, Kept::default(), deadline).now_or_never().unwrap();
        assert_eq!((cut, f.vends, f.kept), (Ok(Ending::Cut), 0, Kept::default()), "no time for the vend: not tried, nothing counted");
    }

    /// Goal: a revoke that keeps failing is given up only after its tries
    /// over a day, then its end kept before the delete, so a deadline or a
    /// failed delete resumes at the delete, and the report says the account
    /// deleted at Pipes only in the call whose delete answered. Method: a
    /// provider that answers `invalid_token`, calls a few hours apart, a
    /// slow vend, and a delete that fails once.
    #[test]
    fn a_given_up_account_is_said_deleted_only_once_it_is() {
        let invalid = (400, json!({ "error": "invalid_token" }));
        let mut f = Fake::new(1_000, invalid);
        for call in 1..REVOKE_TRIES_MAX {
            let w = f.call();
            assert!(!w.done() && w.kept.len() == 1 && w.given_up.is_empty(), "call {call}: kept, the step not done");
            assert_eq!(f.kept.failures.tries, call);
            f.now += REVOKE_GIVE_UP_AFTER_MS / i64::from(REVOKE_TRIES_MAX - 1);
        }
        f.vend_ms = 19_000;
        let gave_up = f.call();
        assert_eq!(f.kept.ended, Some(Ended::GivenUp), "given up, kept so");
        assert!(gave_up.cut && !f.deleted && gave_up.given_up.is_empty(), "past the deadline: not deleted, not said deleted");
        assert!(!gave_up.note().unwrap().contains("deleted at Pipes"));
        f.delete_fails = true;
        assert!(!f.call().done(), "Pipes refused the delete");
        f.delete_fails = false;
        let revokes = f.revokes;
        let deleted = f.call();
        assert!(deleted.done() && f.deleted && f.revokes == revokes, "resumed at the delete");
        assert!(deleted.note().unwrap().contains("deleted at Pipes"), "{:?}", deleted.note());
        assert_eq!(deleted.given_up.len(), 1);
    }

    /// Goal: a vend Pipes answers unreadably never ends an account at once;
    /// it is a failure that counts toward giving it up. Method: a 200 of null.
    #[test]
    fn an_unreadable_vend_keeps_the_account() {
        let mut f = Fake::new(1_000, (200, Value::Null));
        f.vend = (200, Value::Null);
        let w = f.call();
        assert!(!w.done() && !f.deleted && f.revokes == 0 && f.kept.failures.tries == 1, "{w:?}");
        f.vend = (200, json!({ "active": false, "error": "needs_reauthorization" }));
        assert!(f.call().done() && f.deleted, "Pipes saying it holds none ends it");
        assert_eq!(Ended::of_code(Ended::code(Some(Ended::GivenUp))), Ok(Some(Ended::GivenUp)));
        assert_eq!(Ended::of_code(Ended::code(None)), Ok(None));
        assert!(Ended::of_code(3).is_err());
    }

    /// Goal: an account is deleted only once its grant is revoked or gone;
    /// a failed revoke keeps it and fails the step, and only after
    /// `REVOKE_TRIES_MAX` failed calls over a day is it given up (a burst
    /// of retries in one outage never is). Method: each outcome, and the
    /// failures' count and age either side of both bounds.
    #[test]
    fn a_failed_revoke_keeps_the_account_until_it_is_given_up() {
        let failed = Tried::Failed("the provider did not revoke it (503)".into());
        let now = 10 * REVOKE_GIVE_UP_AFTER_MS;
        assert_eq!(then(&Tried::Revoked, Failures::default(), now), Then::Delete);
        assert_eq!(then(&Tried::NoGrant, Failures { tries: 3, first_ms: Some(0) }, now), Then::Delete);
        assert_eq!(then(&Tried::Cut, Failures { tries: REVOKE_TRIES_MAX, first_ms: Some(0) }, now), Then::Cut, "no time to try is no failure");
        assert_eq!(then(&failed, Failures::default(), now), Then::Retry, "the first failure");
        let old = Some(now - REVOKE_GIVE_UP_AFTER_MS);
        assert_eq!(then(&failed, Failures { tries: REVOKE_TRIES_MAX - 2, first_ms: old }, now), Then::Retry, "a day on, too few tries");
        assert_eq!(then(&failed, Failures { tries: REVOKE_TRIES_MAX - 1, first_ms: Some(now - 1000) }, now), Then::Retry, "enough tries, in one burst");
        assert_eq!(then(&failed, Failures { tries: REVOKE_TRIES_MAX - 1, first_ms: old }, now), Then::GiveUp, "enough tries over a day");
        assert_eq!(then(&failed, Failures { tries: u32::MAX, first_ms: Some(i64::MIN) }, i64::MAX), Then::GiveUp, "no overflow");
    }
}
