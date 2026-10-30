//! A hosted door (docs/api.md, "An agent's doors"): a connector fragment its
//! owner binds as one of their agent's doors. It runs the platform's
//! blessed template named for the door (`telegram`), so no editor's code
//! speaks through it: a fork makes its code its own, and the door is shut
//! from then on. Bound, it may publish records that relay its owner to
//! that agent, in its own channels (`fragment_core::door::Hosted`,
//! `effects::check_record`); the agent's bridge takes them as its owner's
//! messages, as it takes a SimpleX door's.
//!
//! Only its owner binds or unbinds it, in person (a signed request of
//! theirs, or their session on its page: `__door`), never an agent,
//! whomever it acts for. The binding is this fragment's (`door_actor`), so
//! a connector's chat is its own and several may each be a door of the same
//! agent. Each publish asks the deployment's connector policy again.

use fragment_core::connectors::Action;
use fragment_core::door::{self, Hosted};
use fragment_core::npub;
use fragment_proto::{ErrorCode, IdentityKind, Role, SetRole};
use serde_json::{json, Value};

use crate::error::{CellError, CellResult};
use crate::fragment::{Caller, FragmentCell, MetaKey};

impl FragmentCell {
    /// The blessed template this fragment's installed code is, if any.
    fn blessed(&self) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT loader_id FROM code WHERE id = 1", vec![])?;
        Ok(rows.first().and_then(|r| r["loader_id"].as_str()).and_then(door::blessed_template).map(str::to_string))
    }

    /// What this fragment may relay now: its binding, while its code is
    /// still the blessed template of its door and the deployment allows it.
    pub(crate) fn hosted_door(&self) -> CellResult<Option<Hosted>> {
        let Some(bound) = self.meta(MetaKey::DoorActor)? else { return Ok(None) };
        let (door, agent) = bound.split_once(' ').ok_or_else(|| CellError::host(format!("its door binding {bound:?} is not `<door> <agent>`")))?;
        if self.blessed()?.as_deref() != Some(door) {
            return Ok(None);
        }
        let owner = self.must(MetaKey::Owner)?;
        if self.cfg.connectors.authorize(&owner, agent, door, Action::Check).is_err() {
            return Ok(None);
        }
        Ok(Some(Hosted { door: door.into(), agent: agent.into(), owner: npub::display(&owner) }))
    }

    /// `GET /api/f/{name}/door` and `GET __door`, its owner's: `{bound:
    /// {door, agent}}`, or `{bound: null}`.
    pub(crate) fn door_binding(&self, caller: &Caller) -> CellResult<Value> {
        self.require(caller, false, Role::Owner)?;
        let bound = self.meta(MetaKey::DoorActor)?.and_then(|b| b.split_once(' ').map(|(door, agent)| json!({ "door": door, "agent": agent })));
        Ok(json!({ "bound": bound }))
    }

    /// Why a fetch of this fragment's job is not made: it is bound as a door
    /// that is shut now (the deployment no longer allows it, or its code is
    /// no longer the connector's), so it sends nothing on its owner's behalf.
    pub(crate) fn door_shut(&self) -> CellResult<Option<String>> {
        let Some(bound) = self.meta(MetaKey::DoorActor)? else { return Ok(None) };
        Ok(self.hosted_door()?.is_none().then(|| format!("this connector is bound as a door that is shut ({bound}): the deployment no longer allows it, or its code is no longer the connector's")))
    }

    /// `POST /api/f/{name}/door` and `POST __door`: `{act: "link", agent}`
    /// binds this connector fragment as `agent`'s door named for its
    /// template, which makes the agent an editor here if it is not a member
    /// (its chat's `work` takes its turns); `{act: "unlink"}` lets it go.
    pub(crate) async fn bind_door(&self, caller: &Caller, body: &Value) -> CellResult<Value> {
        let owner = self.must(MetaKey::Owner)?;
        let in_person = caller.signed.as_ref().is_some_and(|s| s.kind == IdentityKind::Person && s.acting_for.is_none() && s.id == npub::display(&owner));
        if !in_person {
            return Err(CellError::new(ErrorCode::Forbidden, "only its owner, in person, binds a connector as their agent's door: never an agent, whomever it acts for"));
        }
        let text = |k: &str| body[k].as_str().unwrap_or("");
        match text("act") {
            "link" => {}
            "unlink" => {
                if let Some(bound) = self.meta(MetaKey::DoorActor)? {
                    self.del_meta(MetaKey::DoorActor)?;
                    self.event("door.unbound", "no longer its owner's agent's door", json!({ "bound": bound }));
                }
                return Ok(json!({ "ok": true }));
            }
            _ => return Err(CellError::invalid("act: link or unlink")),
        }
        let agent = text("agent");
        if !fragment_proto::valid_fragment_name(agent) {
            return Err(CellError::invalid("agent: the agent fragment's name"));
        }
        let Some(door) = self.blessed()?.filter(|t| fragment_core::connectors::id_ok(t)) else {
            return Err(CellError::new(ErrorCode::Forbidden, "this fragment runs no connector's template: only one does, unforked, as the door of its name"));
        };
        self.cfg.connectors.authorize(&owner, agent, &door, Action::Link).map_err(|why| CellError::new(ErrorCode::Forbidden, format!("door {door} {why}")))?;
        // the agent's fragment says it is theirs, and an agent, and records it
        let asked = json!({ "owner": owner, "door": door, "actor": self.name()? });
        let identity = crate::fragment::ask(&self.env, agent, "computer/door-actor", &asked).await?["identity"].as_str().map(str::to_string).ok_or_else(|| CellError::host("the agent fragment named no identity"))?;
        if self.member_role(&identity)?.is_none() {
            let as_owner = self.as_owner()?;
            self.set_member(&as_owner, &identity, SetRole { role: Role::Editor }).await?;
        }
        self.set_meta(MetaKey::DoorActor, &format!("{door} {agent}"))?;
        self.event("door.bound", &format!("its owner's agent {agent}'s {door} door"), json!({ "door": door, "agent": agent, "identity": identity }));
        Ok(json!({ "door": door, "agent": agent, "relayed": Hosted { door, agent: agent.into(), owner: npub::display(&owner) }.relayed() }))
    }
}
