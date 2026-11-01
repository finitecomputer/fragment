//! A fragment with reach stays private (Paul, 2026-11-01, FIN-352): "as an
//! extra safety right now we should disallow sharing for apps that have
//! reach. When we grow in confidence we can lift that restriction. But for
//! now it should just be the user and their agent, nobody outside."
//!
//! It has reach when it holds a secret (its jobs may send one to a host) or
//! a door (a connector bound as one, doors.rs; an agent's SimpleX door,
//! runs_on.rs). While it has reach it is shared with no one but its owner
//! and their own agents (`Sharing::beyond`), whichever comes first:
//!
//! - sharing it further is refused (403, `Reason::PrivateWhileReaching`):
//!   anyone else added or given a role, an invite or its yes, its link or
//!   public visibility;
//! - giving it reach while it is shared is refused (403,
//!   `Reason::UnshareFirst`): a secret set, a connector bound, an agent's
//!   door linked.
//!
//! What narrows it is never refused (members only, removing someone,
//! revoking an invite, deleting a secret, unlinking), and one shared before
//! the rule keeps serving as it was. Its owner's list rows and its status
//! say it (`reach`), and the share sheet locks (share.rs). A public address
//! is the same fragment's other origin, its members and visibility the
//! same, so it is no sharing here. Temporary: docs/technical-debt-ledger.md.

use fragment_proto::{ErrorCode, Reason};

use crate::error::{CellError, CellResult};
use crate::fragment::{FragmentCell, MetaKey};

/// Why sharing it further is refused.
const PRIVATE: &str = "it holds a key or a door, so for now it stays private to its owner and their own agents: no one else is added or invited, and it has no share link";

/// Why reach is refused.
const UNSHARE_FIRST: &str = "it is shared beyond its owner and their own agents (its link, someone else in it, or an invite waiting), so for now it holds no key and no door: make it private first (`fragment visibility <name> members`, remove the others, revoke the invites)";

impl FragmentCell {
    /// Whether it has reach: a secret, or a door.
    pub(crate) fn reaches(&self) -> CellResult<bool> {
        Ok(self.count("SELECT COUNT(*) AS n FROM secrets")? > 0 || self.meta(MetaKey::DoorActor)?.is_some() || self.meta(MetaKey::Door)?.is_some())
    }

    /// Sharing it further, refused while it has reach.
    pub(crate) fn share_while_private(&self) -> CellResult<()> {
        match self.reaches()? {
            true => Err(CellError::new(ErrorCode::Forbidden, PRIVATE).because(Reason::PrivateWhileReaching)),
            false => Ok(()),
        }
    }

    /// Reach, refused while it is shared beyond its owner and their agents.
    pub(crate) fn reach_while_private(&self) -> CellResult<()> {
        match self.sharing_counts()?.beyond() {
            true => Err(CellError::new(ErrorCode::Forbidden, UNSHARE_FIRST).because(Reason::UnshareFirst)),
            false => Ok(()),
        }
    }
}

/// `GET /api/admin/people/{person}/reach`, an operator's: of the fragments
/// `person` owns, how many of each kind have reach, and how many of those
/// are shared beyond their owner and their agents (from before the rule:
/// they keep serving, nothing more is shared or added). Counts alone, no
/// names; `fragment operator reach` sums them over everyone.
pub(crate) async fn census(env: &worker::Env, person: &str) -> CellResult<serde_json::Value> {
    use std::collections::BTreeMap;
    if !fragment_core::npub::is_identity(person) {
        return Err(CellError::new(ErrorCode::NotFound, format!("no person {person}")));
    }
    let owned: Vec<String> = crate::listed(env, person).await?.fragments.into_iter().filter(|f| f.role == fragment_proto::Role::Owner).map(|f| f.name).collect();
    let none = serde_json::Value::Null;
    let asked = futures_util::future::join_all(owned.iter().map(|name| crate::fragment::ask(env, name, "computer/reach", &none))).await;
    let (mut reaching, mut beyond, mut unanswered) = (BTreeMap::<String, u64>::new(), BTreeMap::<String, u64>::new(), 0u64);
    for answer in asked {
        let Ok(a) = answer else {
            unanswered += 1;
            continue;
        };
        let kind = a["kind"].as_str().unwrap_or("other").to_string();
        if a["reach"] == true {
            *reaching.entry(kind.clone()).or_default() += 1;
            if a["beyond"] == true {
                *beyond.entry(kind).or_default() += 1;
            }
        }
    }
    Ok(serde_json::json!({ "owned": owned.len(), "reaching": reaching, "beyond": beyond, "unanswered": unanswered }))
}
