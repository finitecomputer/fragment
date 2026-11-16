//! A person's list's calls as types: each call's route, its body, and its
//! answer, defined once. The router, fragments and the other objects ask
//! with them (`principal::ask`, or `principal::pass` for an answer passed
//! on unread), and the list decodes the same body and answers the same
//! type (`PrincipalCell::route`), so the two ends agree at compile time, as
//! the Registry's calls (registry/calls.rs) and the ledger's routes do.
//! The one call that is no request and answer, a page's socket, is
//! `principal::watch`.

use fragment_proto::export::{ExportStatus, ExportView};
use fragment_proto::{FragmentKind, FragmentList, ListedRecord, Role, SearchAnswer, Seen, Sharing};
use serde::{Deserialize, Serialize};

/// A call to a list: `PATH` names its route, the implementing type is its
/// body, and `Answer` is what a 200 carries.
pub(crate) trait Call: Serialize {
    const PATH: &'static str;
    type Answer;
}

/// A fragment's change to this identity's row (members.rs `flush_index`,
/// ended.rs `tell_ended`). `incarnation` is the fragment's creation time
/// (a deleted and re-created fragment is a new incarnation); `version`
/// orders changes within one. The owner's row carries the fragment's
/// sharing, as it was when the change was sent.
#[derive(Serialize, Deserialize)]
pub(crate) struct IndexChange {
    pub fragment: String,
    pub role: Option<Role>,
    pub incarnation: i64,
    pub version: i64,
    pub sharing: Option<Sharing>,
    /// What it is and its title, on every row a role names.
    pub face: Option<Face>,
    /// The channels its search holds (those every member may read), on
    /// every row a role names.
    pub searched: Option<Vec<String>>,
    /// What its members are doing (listed.rs in the fragment): the listed
    /// records of the channels this row's role may read, on every row a
    /// role names; none takes away what the row showed.
    pub listed: Vec<ListedRecord>,
}

impl IndexChange {
    /// The change that takes the row away: its fragment's life ended.
    pub(crate) fn gone(fragment: String, incarnation: i64, version: i64) -> IndexChange {
        IndexChange { fragment, role: None, incarnation, version, sharing: None, face: None, searched: None, listed: vec![] }
    }
}

/// Taken: applied when it is newer than the row's, else nothing.
impl Call for IndexChange {
    const PATH: &'static str = "/index";
    type Answer = ();
}

/// What a person's list shows of a fragment, kept as its row's `face`.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Face {
    #[serde(default)]
    pub kind: FragmentKind,
    #[serde(default)]
    pub title: Option<String>,
    /// An agent fragment's picture's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// An agent fragment's role (its `meta.role`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Its agent members, the first added first (`limits::LISTED_AGENTS_MAX`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
}

/// `GET /api/fragments`'s answer, whole: the router passes it on. An
/// agent's (`agent`) says what its messages carry (`Config::uploads_for`).
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct List {
    pub agent: bool,
}

impl Call for List {
    const PATH: &'static str = "/list";
    type Answer = FragmentList;
}

/// The signer's own view of a fragment of theirs, set (`PUT
/// /api/fragments/{name}/archived`, `…/dock`, `…/pinned`): a flag on their
/// row, which changes nothing of the fragment's, nor anyone else's list.
#[derive(Serialize, Deserialize)]
pub(crate) struct SetOwn {
    pub fragment: String,
    pub own: Own,
    pub on: bool,
}

/// The person's own flags on a row.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Own {
    Archived,
    Dock,
    Pinned,
}

impl Call for SetOwn {
    const PATH: &'static str = "/own";
    type Answer = ();
}

/// `PUT /api/fragments/{name}/seen`, for the signer.
#[derive(Serialize, Deserialize)]
pub(crate) struct MarkSeen {
    pub fragment: String,
}

impl Call for MarkSeen {
    const PATH: &'static str = "/seen";
    type Answer = Seen;
}

/// `GET /api/search?q=`, over the signer's list.
#[derive(Serialize, Deserialize)]
pub(crate) struct Search {
    pub q: String,
}

impl Call for Search {
    const PATH: &'static str = "/search";
    type Answer = SearchAnswer;
}

/// A fragment's messages, for this person's search (search.rs sends it).
#[derive(Serialize, Deserialize)]
pub(crate) struct SearchBatch {
    pub fragment: String,
    pub incarnation: i64,
    pub entries: Vec<SearchEntry>,
}

/// One message: its place in its fragment's search log (`n`, from 1,
/// never reused within an incarnation), its record, and its text.
#[derive(Serialize, Deserialize)]
pub(crate) struct SearchEntry {
    pub n: i64,
    pub channel: String,
    pub seq: i64,
    pub at: i64,
    pub text: String,
}

/// What a delivery did: `member` is false when this list holds no role on
/// the fragment at that incarnation (not yet, or not any more), and took
/// nothing; `applied` counts the entries new here.
#[derive(Serialize, Deserialize)]
pub(crate) struct SearchApplied {
    pub member: bool,
    pub applied: u64,
}

impl Call for SearchBatch {
    const PATH: &'static str = "/search/entries";
    type Answer = SearchApplied;
}

/// Something of the person's that their list does not hold changed (what
/// they are told: their computer's notices, the invites waiting on their
/// emails): each open page reads the list, and their notices, again. A
/// page that missed it reads again as it reconnects.
#[derive(Serialize, Deserialize)]
pub(crate) struct Changed {}

impl Call for Changed {
    const PATH: &'static str = "/changed";
    type Answer = ();
}

/// `POST /api/export`: their export started, or the one building (export.rs).
#[derive(Serialize, Deserialize)]
pub(crate) struct ExportStart {
    pub person: String,
}

impl Call for ExportStart {
    const PATH: &'static str = "/export/start";
    type Answer = ExportView;
}

/// `GET /api/export`: their newest export.
#[derive(Serialize, Deserialize)]
pub(crate) struct ExportNow {}

impl Call for ExportNow {
    const PATH: &'static str = "/export/status";
    type Answer = ExportStatus;
}

/// `GET` (`HEAD`: `head`) `/api/export/{id}/download`: its archive, or the
/// `range` of it asked for, as R2 answers it.
#[derive(Serialize, Deserialize)]
pub(crate) struct ExportDownload {
    pub id: String,
    pub head: bool,
    pub range: Option<String>,
}

impl Call for ExportDownload {
    const PATH: &'static str = "/export/download";
    type Answer = worker::Response;
}

/// A wipe's read of the list (wipe.rs): its rows a page at a time, after
/// `after` (a fragment's name), any role or none.
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeView {
    pub after: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WipePage {
    pub rows: Vec<WipeRow>,
    pub more: bool,
}

/// A row as a wipe reads it: any role, or none (a fragment left or ended).
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeRow {
    pub fragment: String,
    pub role: Option<Role>,
}

impl Call for WipeView {
    const PATH: &'static str = "/wipe/view";
    type Answer = WipePage;
}

/// A wipe's end of the list: its sockets closed and every table emptied,
/// with the row that says it was wiped, in one step. Again, it changes
/// nothing.
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeEnd {}

/// The rows it held as it ended.
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeEnded {
    pub rows: u64,
}

impl Call for WipeEnd {
    const PATH: &'static str = "/wipe/end";
    type Answer = WipeEnded;
}

/// A wipe's end of the person's exports (export.rs `wipe_exports`), before
/// the list's own.
#[derive(Serialize, Deserialize)]
pub(crate) struct WipeExports {
    pub person: String,
}

/// `more`: objects are left (call again).
#[derive(Serialize, Deserialize)]
pub(crate) struct ExportsWiped {
    pub more: bool,
}

impl Call for WipeExports {
    const PATH: &'static str = "/wipe/exports";
    type Answer = ExportsWiped;
}

/// Fleets with levers only (`/api/test/list`): a lever on the list
/// (`PrincipalCell::test_lever`).
#[derive(Serialize, Deserialize)]
pub(crate) struct ListLever {
    pub op: String,
}

impl Call for ListLever {
    const PATH: &'static str = "/test/list";
    type Answer = serde_json::Value;
}

/// Fleets with levers only (`/api/test/export`): a lever on the person's
/// export (export.rs `export_lever`).
#[derive(Serialize, Deserialize)]
pub(crate) struct ExportLever {
    pub op: String,
    pub steps: Option<u32>,
}

impl Call for ExportLever {
    const PATH: &'static str = "/test/export";
    type Answer = serde_json::Value;
}
