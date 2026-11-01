//! A hosted door (docs/api.md, "An agent's doors"): a connector fragment its
//! owner binds as one of their agent's doors. Bound, it may publish records
//! that relay its owner to that agent, in its own channels
//! (`fragment_core::door::Hosted`, `effects::check_record`), and the
//! agent's bridge takes them as its owner's messages, as it takes a SimpleX
//! door's. So what it publishes must be its owner's words alone:
//!
//! - **Its code is the platform's.** It is made from a blessed connector
//!   template (`blessed::CONNECTORS`), which marks it (`door_template`)
//!   as it is made; an install of any other code (a fork, a template
//!   changed) unmarks it, lets its binding go and removes its secrets, for
//!   good: a fragment that ever ran other code is no door, and sends
//!   nothing its owner set there (`drop_secrets`).
//! - **No one but its owner edits it.** Every other caller, an agent acting
//!   for its owner included, is at most a viewer there, from its making
//!   (`only_owner_edits`, in `standing`): no one else sets its secrets,
//!   writes its files or pages, deploys its code or calls an editor's
//!   operation. Its own triggered runs keep their role. Nor does anyone
//!   else share it (`agents_share`): no agent sets its visibility, members
//!   or invites, or holds its share link, whomever it acts for.
//! - **Only its owner binds it, in person** (a signed request of theirs, or
//!   their session on its page: `__door`), never an agent, whomever it acts
//!   for. Its agent joins it as a viewer, if it was no member.
//!
//! The binding is this fragment's (`door_actor`), so a connector's chat is
//! its own and several may each be a door of the same agent. Each publish
//! and each fetch asks the deployment's connector policy again, and a job
//! is told its binding (`job.door`).

use std::borrow::Cow;

use fragment_core::access::Standing;
use fragment_core::connectors::{Action, Policy};
use fragment_core::door::{self, Hosted};
use fragment_core::npub;
use fragment_proto::{ErrorCode, IdentityKind, Role, SetRole};
use serde_json::{json, Value};

use crate::error::{CellError, CellResult};
use crate::fragment::{Caller, FragmentCell, MetaKey};

/// A connector's binding as its `door_actor` holds it.
struct Binding {
    door: String,
    /// The agent fragment.
    agent: String,
    /// The agent's identity, which a relayed record is `to`.
    identity: String,
}

impl Binding {
    fn read(stored: &str) -> CellResult<Binding> {
        let parts: Vec<&str> = stored.split(' ').collect();
        let [door, agent, identity] = parts[..] else {
            return Err(CellError::host(format!("its door binding {stored:?} is not `<door> <agent> <identity>`")));
        };
        Ok(Binding { door: door.into(), agent: agent.into(), identity: identity.into() })
    }

    fn stored(&self) -> String {
        format!("{} {} {}", self.door, self.agent, self.identity)
    }
}

impl FragmentCell {
    /// The blessed template this fragment's installed code is, if any.
    fn blessed(&self) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT loader_id FROM code WHERE id = 1", vec![])?;
        Ok(rows.first().and_then(|r| r["loader_id"].as_str()).and_then(door::blessed_template).map(str::to_string))
    }

    /// The connector policy its doors ask: the deployment's, or, on a test
    /// fleet, the `connectors` lever's in its place (ops.rs).
    pub(crate) fn connector_policy(&self) -> CellResult<Cow<'_, Policy>> {
        Ok(match self.meta(MetaKey::TestConnectors)? {
            Some(p) => Cow::Owned(Policy::parse(&p).map_err(|e| CellError::host(format!("the connectors lever: {e}")))?),
            None => Cow::Borrowed(&self.cfg.connectors),
        })
    }

    /// Whether the caller is this fragment's owner, in person: a person
    /// signing (or in their session) as themselves, never an agent.
    fn owner_in_person(&self, caller: &Caller) -> CellResult<bool> {
        let owner = self.must(MetaKey::Owner)?;
        let is_owner = |id: &str| id == owner || id == npub::display(&owner);
        Ok(caller.signed.as_ref().is_some_and(|s| s.kind == IdentityKind::Person && s.acting_for.is_none() && is_owner(&s.id)))
    }

    /// A connector's fragment is its owner's alone to edit: every other
    /// caller, an agent acting for its owner included, is at most a viewer
    /// there (`standing`'s last word).
    pub(crate) fn only_owner_edits(&self, caller: &Caller, mut standing: Standing) -> CellResult<Standing> {
        if self.meta(MetaKey::DoorTemplate)?.is_none() || self.owner_in_person(caller)? {
            return Ok(standing);
        }
        standing.member = standing.member.map(|m| m.min(Role::Viewer));
        if standing.cap.is_some() {
            standing.held = Some(standing.held.map_or(Role::Viewer, |h| h.min(Role::Viewer)));
        }
        Ok(standing)
    }

    /// Whether an agent may share here at all: never on a connector, which
    /// its owner shares in person alone, as they alone edit it (its
    /// visibility, members, invites and share link). Elsewhere an agent
    /// shares for its owner (`access::agent_shares`).
    pub(crate) fn agents_share(&self) -> CellResult<()> {
        if self.meta(MetaKey::DoorTemplate)?.is_some() {
            return Err(CellError::new(ErrorCode::Forbidden, "only its owner, in person, shares a connector: never an agent, whomever it acts for"));
        }
        Ok(())
    }

    /// A role a member may be given here: on a connector, viewer at most, so
    /// its members list says the role that counts (`only_owner_edits`).
    pub(crate) fn connector_role(&self, role: Role) -> CellResult<()> {
        if role > Role::Viewer && self.meta(MetaKey::DoorTemplate)?.is_some() {
            return Err(CellError::invalid("a connector has no editor but its owner: add them as a viewer"));
        }
        Ok(())
    }

    /// Its binding, while it is a door now: made from the template it runs
    /// unforked, and allowed by the deployment.
    fn open_binding(&self) -> CellResult<Option<Binding>> {
        let (Some(stored), Some(template)) = (self.meta(MetaKey::DoorActor)?, self.meta(MetaKey::DoorTemplate)?) else { return Ok(None) };
        let b = Binding::read(&stored)?;
        if b.door != template || self.blessed()?.as_deref() != Some(template.as_str()) {
            return Ok(None);
        }
        let owner = self.must(MetaKey::Owner)?;
        if self.connector_policy()?.authorize(&owner, &b.agent, &b.door, Action::Check).is_err() {
            return Ok(None);
        }
        Ok(Some(b))
    }

    /// What this fragment may relay now (`effects::check_record`).
    pub(crate) fn hosted_door(&self) -> CellResult<Option<Hosted>> {
        let owner = npub::display(&self.must(MetaKey::Owner)?);
        Ok(self.open_binding()?.map(|b| Hosted { door: b.door, agent: b.agent, owner }))
    }

    /// What its jobs are told (`job.door`): the `relayed` they may publish
    /// and the agent it is `to`, or null while it is no open door.
    pub(crate) fn job_door(&self) -> CellResult<Value> {
        let owner = npub::display(&self.must(MetaKey::Owner)?);
        Ok(match self.open_binding()? {
            Some(b) => json!({ "relayed": Hosted { door: b.door, agent: b.agent, owner }.relayed(), "to": b.identity }),
            None => Value::Null,
        })
    }

    /// `GET /api/f/{name}/door` and `GET __door`, its owner's: `{bound:
    /// {door, agent}}`, or `{bound: null}`.
    pub(crate) fn door_binding(&self, caller: &Caller) -> CellResult<Value> {
        self.require(caller, false, Role::Owner)?;
        let bound = match self.meta(MetaKey::DoorActor)? {
            Some(stored) => {
                let b = Binding::read(&stored)?;
                json!({ "door": b.door, "agent": b.agent })
            }
            None => Value::Null,
        };
        Ok(json!({ "bound": bound }))
    }

    /// Why a fetch of this fragment's job is not made: it is bound as a door
    /// that is shut now (the deployment no longer allows it, or its code is
    /// no longer the connector's), so it sends nothing on its owner's behalf.
    pub(crate) fn door_shut(&self) -> CellResult<Option<String>> {
        let Some(stored) = self.meta(MetaKey::DoorActor)? else { return Ok(None) };
        let door = Binding::read(&stored)?.door;
        Ok(self.open_binding()?.is_none().then(|| format!("this connector is bound as a {door} door that is shut: the deployment no longer allows it, or its code is no longer the connector's")))
    }

    /// Its binding let go (an unlink, or other code installed). Its agent
    /// stays a viewer of its chat (a chat's own agent is not removed): it
    /// relays and sends nothing for it from then on (`job.door` is null).
    pub(crate) fn unbind_door(&self, why: &str) -> CellResult<()> {
        let Some(stored) = self.meta(MetaKey::DoorActor)? else { return Ok(()) };
        let b = Binding::read(&stored)?;
        self.del_meta(MetaKey::DoorActor)?;
        self.event("door.unbound", why, json!({ "door": b.door, "agent": b.agent }));
        // its owner's row says whether it still has reach (reach.rs)
        self.sharing_changed()
    }

    /// `POST /api/f/{name}/door` and `POST __door`: `{act: "link", agent}`
    /// binds this connector fragment as `agent`'s door named for its
    /// template, which makes the agent a viewer here if it is not a member
    /// (its chat's `chat` and `work` take its posts); `{act: "unlink"}`
    /// lets it go.
    pub(crate) async fn bind_door(&self, caller: &Caller, body: &Value) -> CellResult<Value> {
        if !self.owner_in_person(caller)? {
            return Err(CellError::new(ErrorCode::Forbidden, "only its owner, in person, binds a connector as their agent's door: never an agent, whomever it acts for"));
        }
        let owner = self.must(MetaKey::Owner)?;
        let text = |k: &str| body[k].as_str().unwrap_or("");
        match text("act") {
            "link" => {}
            "unlink" => {
                self.unbind_door("no longer its owner's agent's door")?;
                return Ok(json!({ "ok": true }));
            }
            _ => return Err(CellError::invalid("act: link or unlink")),
        }
        let agent = text("agent");
        if !fragment_proto::valid_fragment_name(agent) {
            return Err(CellError::invalid("agent: the agent fragment's name"));
        }
        let running = self.blessed()?;
        let Some(door) = self.meta(MetaKey::DoorTemplate)?.filter(|t| running.as_deref() == Some(t.as_str())) else {
            return Err(CellError::new(ErrorCode::Forbidden, "this fragment is no connector: only one made from a connector's template, that never ran other code, is a door"));
        };
        self.connector_policy()?.authorize(&owner, agent, &door, Action::Link).map_err(|why| CellError::new(ErrorCode::Forbidden, format!("door {door} {why}")))?;
        // refused before the agent is asked (it records the binding), and
        // again in the binding's own turn, below
        self.reach_while_private()?;
        // the agent's fragment says it is theirs, and an agent, and records it
        let asked = json!({ "owner": owner, "door": door, "actor": self.name()? });
        let identity = crate::fragment::ask(&self.env, agent, "computer/door-actor", &asked).await?["identity"].as_str().map(str::to_string).ok_or_else(|| CellError::host("the agent fragment named no identity"))?;
        // another agent bound before: its binding goes first
        if self.meta(MetaKey::DoorActor)?.is_some() {
            self.unbind_door("bound to another of its owner's agents")?;
        }
        if self.member_role(&identity)?.is_none() {
            let as_owner = self.as_owner()?;
            self.set_member(&as_owner, &identity, SetRole { role: Role::Viewer }).await?;
        }
        // a door is reach: none while it is shared beyond its owner and
        // their agents (reach.rs), decided again in the binding's own turn
        self.reach_while_private()?;
        let b = Binding { door, agent: agent.into(), identity };
        self.set_meta(MetaKey::DoorActor, &b.stored())?;
        self.sharing_changed()?;
        self.event("door.bound", &format!("its owner's agent {agent}'s {} door", b.door), json!({ "door": b.door, "agent": agent, "identity": b.identity }));
        Ok(json!({ "door": b.door, "agent": agent, "relayed": Hosted { door: b.door.clone(), agent: agent.into(), owner: npub::display(&owner) }.relayed() }))
    }
}
