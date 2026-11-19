//! An agent fragment is its owner's alone (FIN-544; approved 2026-11-18
//! with the apps-as-members design). Its agent sits in its owner's private
//! chats; its `SOUL.md` and `skills/` steer it there and its `memories/`
//! keep what it learnt. So no one but its owner and their own agents is in
//! it or invited, and its link and public never open it, whoever asks: its
//! owner, the agent acting for them, the CLI or the share sheet (which
//! offers none: share.rs).
//!
//! Both ways, as reach.rs: sharing it further is refused, and a fragment
//! shared past its owner is not made an agent (its first run on a
//! computer). It is one by its kind or by the identity it ran as, so a
//! deploy that renames its kind frees nothing. Narrowing is never refused,
//! and one shared before the rule keeps what it had
//! (docs/technical-debt-ledger.md).

use fragment_proto::{ErrorCode, FragmentKind};

use crate::error::{CellError, CellResult};
use crate::fragment::{FragmentCell, MetaKey};

/// Why an agent fragment is shared with no one.
pub(crate) const AGENT: &str = "An agent's own page stays private to its owner.";

/// Why a fragment shared past its owner does not run as an agent.
const SHARED: &str = "It is shared, so it does not run as an agent: make it members only, with no one else in it or invited, first.";

impl FragmentCell {
    /// Why it is its owner's alone, if it is: an agent fragment.
    pub(crate) fn alone(&self) -> CellResult<Option<&'static str>> {
        let agent = self.kind()? == FragmentKind::Agent || self.meta(MetaKey::AgentIdentity)?.is_some();
        Ok(agent.then_some(AGENT))
    }

    /// Sharing it past its owner and their own agents, refused while it is
    /// theirs alone.
    pub(crate) fn share_while_alone(&self) -> CellResult<()> {
        match self.alone()? {
            Some(why) => Err(CellError::new(ErrorCode::Forbidden, why)),
            None => Ok(()),
        }
    }

    /// Whether anyone but its owner and their own agents is in it or
    /// invited, or its link or public opens it: what an agent fragment
    /// shared before the rule still is (an operator's count asks this).
    pub(crate) fn shared_past_owner(&self) -> CellResult<bool> {
        let sharing = self.sharing_counts()?;
        Ok(sharing.with_others() || sharing.beyond())
    }

    /// Made an agent, refused while it is shared past its owner.
    pub(crate) fn alone_while_shared(&self) -> CellResult<()> {
        match self.shared_past_owner()? {
            true => Err(CellError::new(ErrorCode::Forbidden, SHARED)),
            false => Ok(()),
        }
    }
}
