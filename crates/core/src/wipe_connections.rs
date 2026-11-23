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
//!   be gone; a revoke that failed keeps the account (its token is what a
//!   retry revokes with) and fails the step, until it has failed
//!   `REVOKE_TRIES_MAX` calls over at least `REVOKE_GIVE_UP_AFTER_MS`,
//!   when the step gives that account up: deleted at Pipes, said in the
//!   report and the log, the provider's grant perhaps left.

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
    /// or inactive (its provider refused its refresh: `needs_reauthorization`;
    /// or none installed). The provider's grant is gone, or out of reach of
    /// anything Pipes holds.
    NoGrant,
    /// Pipes did not answer, or answered a fault: the grant may live.
    Failed(String),
}

/// Pipes' vend (`POST /data-integrations/{provider}/token`) as a revoke reads it.
pub fn vended(status: u16, answer: &Value) -> Vended {
    match status {
        200 if answer["active"] == true => match answer["access_token"]["access_token"].as_str().filter(|t| !t.is_empty()) {
            Some(t) => Vended::Token(t.to_string()),
            None => Vended::Failed("Pipes vended an active token with none in it".into()),
        },
        200 => Vended::NoGrant,
        404 => Vended::NoGrant,
        s => Vended::Failed(format!("Pipes did not vend a token to revoke ({s})")),
    }
}

/// One account's revoke, tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tried {
    /// The provider confirmed it.
    Revoked,
    /// Nothing left to revoke: Pipes holds no usable grant, the provider
    /// says the token is invalid already, or the row names no revoke.
    NoGrant,
    /// The grant may live: kept, and tried again.
    Failed(String),
}

/// A provider's revocation answer (RFC 7009 §2.2): 200 revoked; Google
/// answers a token revoked or expired already 400 `invalid_token`, which
/// is the grant gone too. Anything else may leave it.
pub fn revoked(status: u16, answer: &Value) -> Tried {
    match status {
        200 => Tried::Revoked,
        400 if answer["error"] == "invalid_token" => Tried::NoGrant,
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

#[cfg(test)]
mod tests {
    use super::*;
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
    /// token to revoke, none left, or a fault that keeps the account.
    /// Method: each answer Pipes and a provider give.
    #[test]
    fn a_try_reads_what_pipes_and_the_provider_said() {
        assert_eq!(vended(200, &json!({ "active": true, "access_token": { "access_token": "ya29.x" } })), Vended::Token("ya29.x".into()));
        assert_eq!(vended(200, &json!({ "active": false, "error": "needs_reauthorization" })), Vended::NoGrant);
        assert_eq!(vended(200, &json!({ "active": false, "error": "not_installed" })), Vended::NoGrant);
        assert_eq!(vended(404, &Value::Null), Vended::NoGrant, "the account gone");
        assert!(matches!(vended(200, &json!({ "active": true, "access_token": { "access_token": "" } })), Vended::Failed(_)));
        for s in [401, 409, 429, 500, 503] {
            assert!(matches!(vended(s, &Value::Null), Vended::Failed(_)), "{s}");
        }
        assert_eq!(revoked(200, &Value::Null), Tried::Revoked);
        assert_eq!(revoked(400, &json!({ "error": "invalid_token", "error_description": "Token expired or revoked" })), Tried::NoGrant, "Google's: gone already");
        for (s, a) in [(400, json!({ "error": "invalid_request" })), (401, Value::Null), (500, Value::Null), (503, json!({ "error": "invalid_token" }))] {
            assert!(matches!(revoked(s, &a), Tried::Failed(_)), "{s} {a}");
        }
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
        assert_eq!(then(&failed, Failures::default(), now), Then::Retry, "the first failure");
        let old = Some(now - REVOKE_GIVE_UP_AFTER_MS);
        assert_eq!(then(&failed, Failures { tries: REVOKE_TRIES_MAX - 2, first_ms: old }, now), Then::Retry, "a day on, too few tries");
        assert_eq!(then(&failed, Failures { tries: REVOKE_TRIES_MAX - 1, first_ms: Some(now - 1000) }, now), Then::Retry, "enough tries, in one burst");
        assert_eq!(then(&failed, Failures { tries: REVOKE_TRIES_MAX - 1, first_ms: old }, now), Then::GiveUp, "enough tries over a day");
        assert_eq!(then(&failed, Failures { tries: u32::MAX, first_ms: Some(i64::MIN) }, i64::MAX), Then::GiveUp, "no overflow");
    }
}
