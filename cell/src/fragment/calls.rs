//! A fragment's calls from inside the platform as types: each call's
//! route, its body, and its answer, defined once. The other objects, the
//! queues' consumers and the router's levers ask with them
//! (`fragment::call`, or `fragment::pass` for an answer passed on unread),
//! and the fragment decodes the same body and answers the same type
//! (`FragmentCell::called`), so the two ends agree at compile time, as a
//! person's list's calls (principal/calls.rs) and the Registry's do.
//!
//! Every call's path is `/call/…`, and it carries `HEADER` (set by
//! `routed::internal_request`), which the router never passes (nor an
//! inbox: `fragment_core::manifest::inbox_header_ok`): a request without it
//! is the router's, and is never decoded as one of these.

use fragment_core::ledger::Metered;
use fragment_core::wipe::LifeLeft;
use fragment_proto::{FragmentKind, IdentityKind, Role, Visibility};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::deliveries::DeliveryKind;

pub(crate) const HEADER: &str = "x-fragment-call";

/// A call to a fragment: `PATH` names its route, the implementing type is
/// its body, and `Answer` is what a 200 carries.
pub(crate) trait Call: Serialize {
    const PATH: &'static str;
    type Answer;
}

/// A record relayed through an agent's door (channels.rs `door_checked`),
/// checked by that agent's fragment: posted by its agent `identity`,
/// through the door `door` its owner opened, its `digest` signed (`sig`)
/// by that door's key. 404 when there is no such fragment.
#[derive(Serialize, Deserialize)]
pub(crate) struct DoorCheck {
    pub identity: String,
    pub door: String,
    pub digest: String,
    pub sig: String,
}

impl Call for DoorCheck {
    const PATH: &'static str = "/call/door/check";
    type Answer = ();
}

/// A connector fragment of its owner's (`actor`), bound by them as the
/// agent's door `door` (doors.rs): it is theirs, and an agent, and says so.
#[derive(Serialize, Deserialize)]
pub(crate) struct DoorActor {
    pub owner: String,
    pub door: String,
    pub actor: String,
}

/// The agent's identity, its fragment's own key's.
#[derive(Serialize, Deserialize)]
pub(crate) struct AgentIdentity {
    pub identity: String,
}

impl Call for DoorActor {
    const PATH: &'static str = "/call/door/actor";
    type Answer = AgentIdentity;
}

/// Its face, for whatever names it: an agent shows by its fragment's
/// (share.rs `name_agents`).
#[derive(Serialize, Deserialize)]
pub(crate) struct FaceOf {}

/// Its title, and an agent's picture's address; none for one it has not.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Named {
    pub title: Option<String>,
    pub picture: Option<String>,
}

impl Call for FaceOf {
    const PATH: &'static str = "/call/face";
    type Answer = Named;
}

/// Its reach, and whether its link opens it, for an operator's census
/// (reach.rs): no names.
#[derive(Serialize, Deserialize)]
pub(crate) struct ReachOf {}

#[derive(Serialize, Deserialize)]
pub(crate) struct Reach {
    pub kind: FragmentKind,
    pub reach: bool,
    pub beyond: bool,
}

impl Call for ReachOf {
    const PATH: &'static str = "/call/reach";
    type Answer = Reach;
}

/// Who may read it now, for a frame's redemption (auth.rs `anyone_reads`).
#[derive(Serialize, Deserialize)]
pub(crate) struct VisibilityOf {}

impl Call for VisibilityOf {
    const PATH: &'static str = "/call/visibility";
    type Answer = Visibility;
}

/// An agent fragment's picture, for `GET /api/identities/<npub>/picture`:
/// its bytes, as the fragment streams them (runs_on.rs `picture`).
#[derive(Serialize, Deserialize)]
pub(crate) struct Picture {}

impl Call for Picture {
    const PATH: &'static str = "/call/picture";
    type Answer = worker::Response;
}

/// `agent`, just made by `person` (members.rs `agent_made`), comes in
/// beside them when they are a member here who is not its owner, at their
/// role. Anything else changes nothing.
#[derive(Serialize, Deserialize)]
pub(crate) struct AgentMade {
    pub person: String,
    pub agent: String,
}

impl Call for AgentMade {
    const PATH: &'static str = "/call/agent-made";
    type Answer = ();
}

/// The invite waiting on `email` here, answered by the person `identity`,
/// who signs in as it, verified (registry/invites.rs): `yes`, they accept
/// it (or, for an invite from before invites waited for a yes, signed in
/// as it and meet it), and their `agents` come in with them (decision 64),
/// at most `AGENTS_PER_OWNER_MAX`; no, they decline it. 404 when none
/// waits. A yes answers the role they hold now; a no, none.
#[derive(Serialize, Deserialize)]
pub(crate) struct InviteAnswer {
    pub email: String,
    pub identity: String,
    pub agents: Vec<String>,
    pub yes: bool,
}

impl Call for InviteAnswer {
    const PATH: &'static str = "/call/invite";
    type Answer = Option<Role>;
}

/// A wipe's end of a fragment its person `owner` owns (wipe.rs; ended.rs
/// `wipe_owned`): its life ended, its repo with it, and what one pass of
/// its alarm would clean, cleaned now. The lists of `owner` and `agents`
/// are not waited on: the wipe empties them. 403 for someone else's.
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeOwned {
    pub owner: String,
    pub agents: Vec<String>,
}

/// Whether a life ended now, and the ended lives still to clean (`left`;
/// 0: done) with what each has left (`lives`).
#[derive(Serialize, Deserialize)]
pub(crate) struct OwnedEnded {
    pub ended: bool,
    pub left: u64,
    pub lives: Vec<LifeLeft>,
}

impl Call for WipeOwned {
    const PATH: &'static str = "/call/wipe/owned";
    type Answer = OwnedEnded;
}

/// A wipe's leave (wipe.rs; ended.rs `wipe_membership`): the wiped person,
/// or an agent of theirs, leaves this fragment of someone else's, as a
/// member leaves. Answers whether they were a member.
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeMembership {
    pub principal: String,
    pub kind: IdentityKind,
}

impl Call for WipeMembership {
    const PATH: &'static str = "/call/wipe/membership";
    type Answer = bool;
}

/// What the delivery consumer tells a fragment of one of its deliveries
/// (deliveries.rs `delivery_report`).
#[derive(Serialize, Deserialize)]
pub(crate) struct Report {
    pub incarnation: String,
    pub kind: DeliveryKind,
    pub url: String,
    pub sub: Option<i64>,
    pub outcome: Outcome,
    /// The receiver's answer (0: none came).
    pub status: u16,
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// The receiver dropped the subscription (404 or 410): the fragment drops it too.
    Gone,
    /// The delivery failed for good.
    Failed,
}

impl Call for Report {
    const PATH: &'static str = "/call/delivery/report";
    type Answer = ();
}

/// What the meter's consumer tells the fragment its batch `seq` came to
/// (meter.rs `meter_acked`).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Acked {
    pub seq: i64,
    pub batch: String,
    /// The ledger's answer, or why it never takes the batch (a guest
    /// pays for nothing: their fragments are not billed).
    pub metered: Option<Metered>,
    pub refused: Option<String>,
}

impl Call for Acked {
    const PATH: &'static str = "/call/meter/acked";
    type Answer = ();
}

/// Whose the fragment is, for the model route (models.rs): a call names a
/// fragment its agent `principal` is in.
#[derive(Serialize, Deserialize)]
pub(crate) struct Whose {
    pub principal: String,
}

/// The fragment's owner, and whether the agent naming it is a member.
#[derive(Serialize, Deserialize)]
pub(crate) struct Owned {
    pub owner: String,
    pub member: bool,
}

impl Call for Whose {
    const PATH: &'static str = "/call/whose";
    type Answer = Owned;
}

/// The finite.computer move's routine (migrations.rs), appended to this
/// agent fragment's own `tasks` once by its `key` (runs_on.rs `move_routine`).
#[derive(Serialize, Deserialize)]
pub(crate) struct Routine {
    pub key: String,
    pub text: String,
}

impl Call for Routine {
    const PATH: &'static str = "/call/move/routine";
    type Answer = ();
}

/// Fleets with levers only (`/api/test/fragment`): a lever on the fragment
/// (ops.rs `test_fragment`), its body the test's.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct FragmentLever(pub Value);

impl Call for FragmentLever {
    const PATH: &'static str = "/call/test/fragment";
    type Answer = Value;
}

/// Fleets with levers only (`/api/test/keys`): this cell's own sealing,
/// `{op: seal, plaintext}` or `{op: open, sealed}`.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct KeysLever(pub Value);

impl Call for KeysLever {
    const PATH: &'static str = "/call/test/keys";
    type Answer = Value;
}
