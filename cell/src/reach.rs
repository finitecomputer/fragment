//! A fragment with reach has no share link and is not public (Paul,
//! 2026-11-01, FIN-352): "as an extra safety right now we should disallow
//! sharing for apps that have reach". Narrowed 2026-11-11: "this needs to
//! only apply to public and anyone with the link. restricted will be used
//! for brain sharing and brains really need those connections for
//! workflows." The people its owner names, at any role, may use its keys,
//! and the share sheet says so to its owner, once.
//!
//! It has reach when it holds a secret (its jobs may send one to a host) or
//! a door (a connector bound as one, doors.rs; an agent's SimpleX door,
//! runs_on.rs). While it has reach its link does not open it
//! (`Sharing::beyond`), whichever comes first:
//!
//! - its link or public visibility is refused (403,
//!   `Reason::PrivateWhileReaching`);
//! - giving it reach while its link opens it is refused (403,
//!   `Reason::UnshareFirst`): a secret set, a connector bound, an agent's
//!   door linked.
//!
//! People named (added, given a role, invited, an invite's yes) are never
//! refused, nor what narrows it (members only, deleting a secret,
//! unlinking), and one opened by its link before the rule keeps serving as
//! it was. Its owner's list rows and its status say it (`reach`), and the
//! share sheet offers members only (share.rs). A public address is the
//! same fragment's other origin, its members and visibility the same, so it
//! is no sharing here. Temporary: docs/technical-debt-ledger.md.

use fragment_proto::{ErrorCode, Reason};

use crate::error::{CellError, CellResult};
use crate::fragment::{FragmentCell, MetaKey};

/// Why its link or public is refused.
const PRIVATE: &str = "it holds a key or a door, so for now it is not shared by link or made public: add people by name instead";

/// Why reach is refused.
const UNSHARE_FIRST: &str = "it is shared by link or public, so for now it holds no key and no door: make it members only first (`fragment visibility <name> members`)";

impl FragmentCell {
    /// Whether it has reach: a secret, or a door.
    pub(crate) fn reaches(&self) -> CellResult<bool> {
        Ok(self.count("SELECT COUNT(*) AS n FROM secrets")? > 0 || self.meta(MetaKey::DoorActor)?.is_some() || self.meta(MetaKey::Door)?.is_some())
    }

    /// Its owner's row told its reach, once: a fragment from before
    /// `Sharing.reach` (its rows sent without it) sends its row again on
    /// its first request after the release, so a key or a door set then
    /// shows (the census, touching each, brings every row current).
    pub(crate) fn reach_told(&self) -> CellResult<()> {
        if self.meta(MetaKey::ReachTold)?.is_some() || self.meta(MetaKey::Owner)?.is_none() {
            return Ok(());
        }
        if self.reaches()? {
            self.sharing_changed()?;
        }
        self.set_meta(MetaKey::ReachTold, "1")
    }

    /// Its link or public, refused while it has reach.
    pub(crate) fn share_while_private(&self) -> CellResult<()> {
        match self.reaches()? {
            true => Err(CellError::new(ErrorCode::Forbidden, PRIVATE).because(Reason::PrivateWhileReaching)),
            false => Ok(()),
        }
    }

    /// Reach, refused while its link opens it.
    pub(crate) fn reach_while_private(&self) -> CellResult<()> {
        match self.sharing_counts()?.beyond() {
            true => Err(CellError::new(ErrorCode::Forbidden, UNSHARE_FIRST).because(Reason::UnshareFirst)),
            false => Ok(()),
        }
    }
}

/// `GET /api/admin/people/{person}/reach`, an operator's: of the fragments
/// `person` owns, how many of each kind have reach, and how many of those
/// are open by their link or public (from before the rule: they keep
/// serving, and stay so until made members only). Counts alone, no
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
