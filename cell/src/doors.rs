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
//!   (`permit::decide`'s connector rule): no one else sets its secrets,
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
//!
//! **A SimpleX connector's door ends with it** (cell/door.mjs `/door/end`:
//! its container, its saves, its storage): its owner's unlink ends it
//! before the binding goes (and fails, bound still, while it cannot); its
//! deletion, or other code installed, records it for the alarm to end
//! (ended.rs). Bound to another of its owner's agents, it stays: its link
//! is its owner's, whichever agent it reaches.

use std::borrow::Cow;

use fragment_core::connectors::{Action, Policy};
use fragment_core::door::{self, Hosted};
use fragment_core::ended::{self as rules, Failure};
use fragment_core::npub;
use fragment_proto::{ErrorCode, IdentityKind, Role, SetRole};
use serde_json::{json, Value};

use crate::error::{CellError, CellResult};
use crate::fragment::calls::DoorActor;
use crate::fragment::{Caller, FragmentCell, MetaKey};
use crate::routed::Signed;

/// Ends a SimpleX connector's door (cell/door.mjs `/door/end`): its
/// container with no last save, its files under its prefix and its
/// object's storage. Ended already, it ends again: so a try is safe to
/// repeat. `last`: its connector's life is over (deleted, wiped, other
/// code), and the door never starts again, whatever connect comes late;
/// else (unbound) a connect starts a new one. Its answer told apart
/// (`fragment_core::ended::door_ended`).
pub(crate) async fn end_door(env: &worker::Env, door: &str, last: bool) -> Result<(), Failure> {
    // `door_id` makes it: a name and hex digits, nothing to escape
    assert!(door::door_of(door).is_some(), "a door's end names a connector's door: {door}");
    let body = format!(r#"{{"door":"{door}","final":{last}}}"#);
    let asked = crate::routed::internal_request("door/end", &body).map_err(|e| Failure::Transient(rules::kept_error(&e.message)))?;
    match crate::routed::send(env, "SIMPLEX_DOOR", door, asked).await {
        Ok(mut answer) => {
            let status = answer.status_code();
            let said = answer.text().await.unwrap_or_default();
            rules::door_ended(status, &said)
        }
        // its object deploying (another release's), or not reached
        Err(e) => Err(Failure::Transient(rules::kept_error(&format!("{:?}: {}", e.code, e.message)))),
    }
}

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

/// Whether `signed` is the owner a fragment was made by (`owner`, its
/// `Owner` row), in person: a person signing (or in their session) as
/// themselves, by that id or its display, never an agent.
pub(crate) fn owner_in_person(signed: &Signed, owner: &str) -> bool {
    signed.kind == IdentityKind::Person && signed.acting_for.is_none() && (signed.id == owner || signed.id == npub::display(owner))
}

impl FragmentCell {
    /// The blessed template this fragment's installed code is, if any.
    pub(crate) fn blessed(&self) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT loader_id FROM code WHERE id = 1", vec![])?;
        Ok(rows.first().and_then(|r| r["loader_id"].as_str()).and_then(door::blessed_template).map(str::to_string))
    }

    /// Whether this fragment runs a connector's own code: made from a
    /// blessed connector template (`door_template`, unmarked for good by
    /// any other install) and running it now.
    pub(crate) fn runs_connector(&self) -> CellResult<bool> {
        let Some(template) = self.meta(MetaKey::DoorTemplate)? else { return Ok(false) };
        Ok(self.blessed()?.as_deref() == Some(template.as_str()))
    }

    /// The connector policy its doors ask: the deployment's, or, on a test
    /// fleet, the `connectors` lever's in its place (ops.rs).
    pub(crate) fn connector_policy(&self) -> CellResult<Cow<'_, Policy>> {
        Ok(match self.meta(MetaKey::TestConnectors)? {
            Some(p) => Cow::Owned(Policy::parse(&p).map_err(|e| CellError::host(format!("the connectors lever: {e}")))?),
            None => Cow::Borrowed(&self.cfg.connectors),
        })
    }

    /// Whether an agent may share here at all: never on a connector, which
    /// its owner shares in person alone, as they alone edit it (its
    /// visibility, members, invites and share link). Elsewhere an agent
    /// shares for its owner (`permit::agent_shares`).
    pub(crate) fn agents_share(&self) -> CellResult<()> {
        if self.meta(MetaKey::DoorTemplate)?.is_some() {
            return Err(CellError::new(ErrorCode::Forbidden, "only its owner, in person, shares a connector: never an agent, whomever it acts for"));
        }
        Ok(())
    }

    /// A role a member may be given here: on a connector, viewer at most, so
    /// its members list says the role that counts (`permit::decide`).
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

    /// The agent this connector is an open door of, as its members list
    /// names it (npub), and the door: that agent's row says it
    /// (`member_list`), so the agent's bridge and pages know which records
    /// relaying its owner came through its door (docs/chat-records.md).
    pub(crate) fn door_member(&self) -> CellResult<Option<(String, String)>> {
        Ok(self.open_binding()?.map(|b| (npub::display(&b.identity), b.door)))
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

    /// This life's SimpleX door, if it is a SimpleX connector's
    /// (`door_template`): its name and its life's digits (`door::door_id`).
    pub(crate) fn own_door(&self) -> CellResult<Option<String>> {
        if self.meta(MetaKey::DoorTemplate)?.as_deref() != Some(door::SIMPLEX) {
            return Ok(None);
        }
        Ok(door::door_id(&self.name()?, &self.must(MetaKey::Npub)?))
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
        let owner = self.must(MetaKey::Owner)?;
        if !caller.signed.as_ref().is_some_and(|s| owner_in_person(s, &owner)) {
            return Err(CellError::new(ErrorCode::Forbidden, "only its owner, in person, binds a connector as their agent's door: never an agent, whomever it acts for"));
        }
        let text = |k: &str| body[k].as_str().unwrap_or("");
        match text("act") {
            "link" => {}
            "unlink" => {
                // its door ends first: an unlink that cannot end it leaves
                // it bound, to be asked again. Bound again, it may connect
                // again: not its last end. Its life is read again after:
                // one made since under its name is another's, unbound by
                // no one but its owner
                if let Some(own) = self.own_door()? {
                    end_door(&self.env, &own, false).await.map_err(|f| CellError::new(ErrorCode::UpstreamFailed, format!("its SimpleX door did not end, so it is still linked: {}", f.message())))?;
                    if self.meta(MetaKey::DoorTemplate)?.as_deref() != Some(door::SIMPLEX) || self.meta(MetaKey::Npub)?.and_then(|npub| door::door_id(&self.name().ok()?, &npub)).as_deref() != Some(own.as_str()) {
                        return Err(CellError::new(ErrorCode::Conflict, "this connector was deleted or made again while its door ended: nothing unlinked"));
                    }
                }
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
        let asked = DoorActor { owner: owner.clone(), door: door.clone(), actor: self.name()? };
        let identity = crate::fragment::call(&self.env, agent, &asked).await?.identity;
        // another agent bound before: its binding goes first
        if self.meta(MetaKey::DoorActor)?.is_some() {
            self.unbind_door("bound to another of its owner's agents")?;
        }
        if self.member_role(&identity)?.is_none() {
            let as_owner = self.as_owner()?;
            self.set_member(&as_owner, &identity, SetRole { role: Role::Viewer }).await?;
        }
        // a door is reach: none while its link opens it (reach.rs), decided
        // again in the binding's own turn
        self.reach_while_private()?;
        let b = Binding { door, agent: agent.into(), identity };
        self.set_meta(MetaKey::DoorActor, &b.stored())?;
        self.sharing_changed()?;
        self.event("door.bound", &format!("its owner's agent {agent}'s {} door", b.door), json!({ "door": b.door, "agent": agent, "identity": b.identity }));
        Ok(json!({ "door": b.door, "agent": agent, "relayed": Hosted { door: b.door.clone(), agent: agent.into(), owner: npub::display(&owner) }.relayed() }))
    }
}
