//! A fragment's calls from inside the platform as types: each call's
//! route, its body, and its answer, defined once. The other objects, the
//! queues' consumers and the router's levers ask with them
//! (`fragment::call`, or `fragment::pass` for an answer passed on unread),
//! and the fragment decodes the same body and answers the same type
//! (`FragmentCell::called`), so the two ends agree at compile time, as a
//! person's list's calls (principal/calls.rs) and the Registry's do.
//!
//! entry.mjs makes some of them too: a run's Workflow (`Job`) and the app
//! facet's capabilities (`Files`, `Search`), naming the same paths through
//! `routed::InternalRoute`; JavaScript cannot share these types, so the
//! e2e's jobs, files and search sections hold the two ends together.
//!
//! Every call's path is `/call/…`, and it carries `HEADER` (set by
//! `routed::internal_request`), which the router never passes (nor an
//! inbox: `fragment_core::manifest::inbox_header_ok`): a request without it
//! is the router's, and is never decoded as one of these.

use fragment_core::ledger::Metered;
use fragment_core::wipe::LifeLeft;
use fragment_proto::search;
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

/// A fragment, the target, asks this one, an app, in at `role`, for their
/// one owner `owner` (app_members.rs), at the target's life `incarnation`
/// and change `version`: the app answers who it shows, and records that it
/// is joining in the same turn; or refuses, saying why in one line.
#[derive(Serialize, Deserialize)]
pub(crate) struct AppAsked {
    pub target: String,
    /// The target's title, as the app's refusals name it.
    pub title: String,
    pub incarnation: i64,
    pub version: i64,
    pub owner: String,
    pub role: Role,
}

/// The app's own key, its life, its title, and its audience
/// (`fragment_core::app_member::audience`): each person, with what they do
/// to it.
#[derive(Serialize, Deserialize)]
pub(crate) struct AppAnswer {
    pub npub: String,
    pub incarnation: i64,
    pub title: String,
    pub audience: Vec<(String, Role)>,
}

impl Call for AppAsked {
    const PATH: &'static str = "/call/app/asked";
    type Answer = AppAnswer;
}

/// The app's audience as a fragment it is in holds it: its people, an
/// agent as its person, with what each does to it.
#[derive(Serialize, Deserialize)]
pub(crate) struct AppAudienceOf {}

impl Call for AppAudienceOf {
    const PATH: &'static str = "/call/app/audience";
    type Answer = Vec<(String, Role)>;
}

/// A change between an app and a target, as a list takes one
/// (`IndexChange`, by the fragment that sends it): the target's word on the
/// app's role there (none: it is out); or, `from_app`, the app's: at its
/// life's end, that it is gone (none), and at a change to who is in it, to
/// look at its audience again (`viewer`). 404 from a fragment with no
/// life: nothing to take.
#[derive(Serialize, Deserialize)]
pub(crate) struct AppTold {
    pub from_app: bool,
    pub change: crate::principal::calls::IndexChange,
}

impl Call for AppTold {
    const PATH: &'static str = "/call/app/told";
    type Answer = ();
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

/// Agent `identity`, this agent fragment's, was added to `fragment`, its
/// membership made at `at` (runs_on.rs `flush_joined`): when it runs on a
/// computer, its `tasks` say so (once by the membership: a retry posts
/// nothing twice). Answers the computer it runs on, for its caller to wake;
/// none for an agent that runs on none, or is not this fragment's.
#[derive(Serialize, Deserialize)]
pub(crate) struct AgentJoined {
    pub identity: String,
    pub fragment: String,
    pub at: i64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct JoinedAnswer {
    pub computer: Option<String>,
    /// Whether its `tasks` say so: false where it declares none to post to.
    pub posted: bool,
}

impl Call for AgentJoined {
    const PATH: &'static str = "/call/agent-joined";
    type Answer = JoinedAnswer;
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

/// A run's Workflow (entry.mjs `Job`; jobs.rs) drives it with three calls,
/// each naming the fragment's life it is from (`incarnation`) and the run
/// and attempt it drives: one from an earlier life, or for a run or
/// attempt no longer current, does nothing and is told `{stop: true}`.
///
/// An advance: the job's body re-run in the app facet over the answers of
/// its first `count` steps, up to its next step.
#[derive(Serialize, Deserialize)]
pub(crate) struct JobAdvance {
    pub incarnation: String,
    pub run: i64,
    pub attempt: u32,
    /// Steps taken so far: the cell kept each one's answer.
    pub count: u32,
    /// The last step, when it ran out of retries: its answer is the error
    /// only the Workflow saw.
    #[serde(default)]
    pub failed: Option<FailedStep>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FailedStep {
    pub index: u32,
    pub kind: String,
    pub error: String,
}

impl Call for JobAdvance {
    const PATH: &'static str = "/call/job/advance";
    /// The next step, or how the run ended.
    type Answer = Value;
}

/// A step, performed and its answer kept: its kind and args decode into a
/// `fragment_core::steps::Step` (a step that does not is the job's failure).
#[derive(Serialize, Deserialize)]
pub(crate) struct JobEffect {
    pub incarnation: String,
    pub run: i64,
    pub attempt: u32,
    pub index: u32,
    pub kind: String,
    pub args: Value,
    /// The Workflow's try of this step, from 1 (its step context's
    /// `attempt`), when it says: a text step's timing reports it.
    #[serde(default, rename = "try")]
    pub tried: Option<u32>,
}

impl Call for JobEffect {
    const PATH: &'static str = "/call/job/effect";
    /// The step's answer, or `{stop: true}`.
    type Answer = Value;
}

/// The Workflow gave up: the run is held with its `error`.
#[derive(Serialize, Deserialize)]
pub(crate) struct JobFinish {
    pub incarnation: String,
    pub run: i64,
    pub attempt: u32,
    pub error: String,
}

impl Call for JobFinish {
    const PATH: &'static str = "/call/job/finish";
    type Answer = ();
}

/// The app facet's `Files` capability (entry.mjs `Files`; files.rs): a
/// file's bytes at `main`, 404 when there is none.
#[derive(Serialize, Deserialize)]
pub(crate) struct FilesRead {
    pub path: String,
}

impl Call for FilesRead {
    const PATH: &'static str = "/call/files/read";
    type Answer = worker::Response;
}

/// The files at `main` under `prefix`.
#[derive(Serialize, Deserialize)]
pub(crate) struct FilesList {
    pub prefix: String,
}

impl Call for FilesList {
    const PATH: &'static str = "/call/files/list";
    type Answer = Vec<Value>;
}

/// A file at `main`: its size, blob and last commit; none when absent.
#[derive(Serialize, Deserialize)]
pub(crate) struct FilesStat {
    pub path: String,
}

impl Call for FilesStat {
    const PATH: &'static str = "/call/files/stat";
    type Answer = Option<Value>;
}

/// The app facet's `Search` capability (entry.mjs `Search`; searchable.rs):
/// its own index, written and asked with the API's bodies, and answered as
/// the API answers them.
impl Call for search::Upsert {
    const PATH: &'static str = "/call/search/upsert";
    type Answer = worker::Response;
}

impl Call for search::Delete {
    const PATH: &'static str = "/call/search/delete";
    type Answer = worker::Response;
}

impl Call for search::Query {
    const PATH: &'static str = "/call/search/query";
    type Answer = worker::Response;
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SearchStatus {}

impl Call for SearchStatus {
    const PATH: &'static str = "/call/search/status";
    type Answer = worker::Response;
}

/// The capabilities' calls: each is one of the app facet's subrequests
/// (ops.rs `facet_calls_lever`).
pub(crate) const CAPABILITIES: [&str; 7] =
    [FilesRead::PATH, FilesList::PATH, FilesStat::PATH, search::Upsert::PATH, search::Delete::PATH, search::Query::PATH, SearchStatus::PATH];
