//! The move of finite.computer's agents to this platform (FIN-463;
//! docs/migrations.md): what the mover, the new agent and the operators
//! send and read of the `Migrations` object, which owns each old agent's
//! move. Removable whole with it (docs/technical-debt-ledger.md, "The
//! finite.computer move"): its routes are retired in `wire.retired`.
//!
//! - **The mover** (finite-mono's `finite-mover`, which only calls out)
//!   signs with its own key (NIP-98), the one the deployment's
//!   `migrations.mover` names: it pushes the roster, pulls its mailbox and
//!   reports on the command it was given. It acts on nothing it was not
//!   told to.
//! - **The new agent** reads its own stage while it imports: the
//!   manifest, then the stage's zip by ranges, then says it is done.
//! - **Operators** list the moves and act on a held one (not on the wire:
//!   the admin's routes never are). That is the whole admin surface.
//!
//! The stage is one zip64 per old agent and attempt, `<prefix>stage.zip`,
//! with `<prefix>manifest.json` beside it, written last: its presence is
//! what staged means (`STAGE_FORMAT`).

use serde::{Deserialize, Serialize};

/// An old agent's id: Core's opaque project id, which names it in every
/// key and path (never a name or an email): 1 to this of `A-Z a-z 0-9 _ -`.
pub const OLD_ID_MAX_BYTES: usize = 64;
/// People in one roster: the old platform has about 150 owners.
pub const ROSTER_PEOPLE_MAX: usize = 2_000;
/// Old agents one person owns, in a roster: 3 at most today.
pub const ROSTER_AGENTS_MAX: usize = 16;
/// Brains, and sites, listed for one old agent.
pub const ROSTER_LIST_MAX: usize = 256;
/// A short text the mover sends (a snapshot's id, a step, a receipt, a
/// title, an email) and a failure's why, in bytes.
pub const TEXT_MAX_BYTES: usize = 512;
/// Commands one mailbox read answers at most: the rest wait for the next.
pub const MAILBOX_MAX: usize = 64;
/// Attempts at one move's stage before it is held for staff.
pub const ATTEMPTS_MAX: u32 = 3;
/// `manifest.json`, read whole by the platform and the importer.
pub const MANIFEST_MAX_BYTES: usize = 256 * 1024;
/// Paths a manifest lists as left behind, at most (`left_more` counts the rest).
pub const MANIFEST_LEFT_MAX: usize = 1_000;
/// The stage's format, as its manifest names it.
pub const STAGE_FORMAT: &str = "finite-stage/1";
/// The stage's zip and its manifest, under its prefix.
pub const STAGE_ZIP: &str = "stage.zip";
pub const STAGE_MANIFEST: &str = "manifest.json";
/// A part of a stage's zip, at most: under a request's body limit (100 MB
/// on Cloudflare's Free and Pro plans), streamed into R2 unread.
pub const PART_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// R2's least part, every part but the last.
pub const PART_MIN_BYTES: u64 = 5 * 1024 * 1024;
/// Parts of one stage, at most (R2's): 640 GB at `PART_MAX_BYTES`.
pub const PARTS_MAX: u32 = 10_000;

/// Whether `id` is an old agent's id (`OLD_ID_MAX_BYTES`).
pub fn valid_old(id: &str) -> bool {
    !id.is_empty() && id.len() <= OLD_ID_MAX_BYTES && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The prefix of an attempt's stage in the platform's `STAGE` bucket:
/// `p1/<old>/<attempt>/`. Each attempt has its own, so a retry never mixes
/// with a half-written one. Only the platform names it: the mover uploads
/// by the stage's id.
pub fn stage_prefix(phase: Phase, old: &str, attempt: u32) -> String {
    assert!(valid_old(old), "an old agent's id is checked before a prefix names it");
    format!("{}/{old}/{attempt}/", phase.as_str())
}

/// A stage's id, as the new agent names it (`/api/migrations/stages/{stage}`
/// and its routine): `<old>.<phase>.<attempt>`.
pub fn stage_id(phase: Phase, old: &str, attempt: u32) -> String {
    assert!(valid_old(old), "an old agent's id is checked before a stage names it");
    format!("{old}.{}.{attempt}", phase.as_str())
}

/// A stage's id read back: its old agent, phase and attempt.
pub fn parse_stage(stage: &str) -> Option<(String, Phase, u32)> {
    let mut parts = stage.rsplitn(3, '.');
    let attempt = parts.next()?;
    let phase = parts.next()?;
    let old = parts.next()?;
    let phase = match phase {
        "p1" => Phase::P1,
        "p2" => Phase::P2,
        _ => return None,
    };
    let attempt = attempt.parse::<u32>().ok().filter(|a| *a >= 1 && attempt == a.to_string())?;
    valid_old(old).then(|| (old.to_string(), phase, attempt))
}

/// A command's id: `<old>:<attempt>:<step>`, what the mover keys its work
/// by. Its step is its kind (`stop_and_stage`, `restart`), or a
/// retirement's with its mark (`retire_migrated`, `retire_archived`): phase
/// 2 retires an old agent twice in one attempt, as archived and then, after
/// a late import, as migrated, and no two commands share an id.
pub fn command_id(old: &str, attempt: u32, step: &str) -> String {
    format!("{old}:{attempt}:{step}")
}

/// Which phase a stage is of: one the person asked for (`p1`), or one we
/// made at the cutoff for someone who never answered (`p2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    P1,
    P2,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::P1 => "p1",
            Phase::P2 => "p2",
        }
    }
}

/// Where one old agent's move is (docs/migrations.md, "The state machine").
///
/// Open (`x-fragment-open`, docs/wire): a reader (the mover) takes a
/// state it does not know as one it does not act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(extend("x-fragment-open" = true)))]
#[serde(rename_all = "snake_case")]
pub enum MoveState {
    /// On the roster; its person is not known here yet.
    Listed,
    /// Offered to its person.
    Open,
    /// "Not now": offered again a day later.
    Later,
    /// "Never": not offered, until they ask again.
    Never,
    /// "Yes": the mover is told to stop and stage it.
    Asked,
    /// The mover stopped it and is staging it.
    Staging,
    /// Staged: its new agent is bringing itself over.
    Importing,
    /// Imported: the mover is told to retire the old one.
    Retiring,
    /// Done: the old agent is retired and marked migrated.
    Moved,
    /// Stuck: staff take it from here (`HeldReason`).
    Held,
    /// Phase 2: staged and retired at the cutoff, unasked.
    Archiving,
    /// Phase 2: its stage is kept 90 days, for a late Yes.
    Archived,
    /// Phase 2: its 90 days passed; its stage is deleted.
    Expired,
    /// Gone from the roster before it was asked (moved by hand, or no
    /// longer live).
    Withdrawn,
}

impl MoveState {
    pub fn as_str(self) -> &'static str {
        match self {
            MoveState::Listed => "listed",
            MoveState::Open => "open",
            MoveState::Later => "later",
            MoveState::Never => "never",
            MoveState::Asked => "asked",
            MoveState::Staging => "staging",
            MoveState::Importing => "importing",
            MoveState::Retiring => "retiring",
            MoveState::Moved => "moved",
            MoveState::Held => "held",
            MoveState::Archiving => "archiving",
            MoveState::Archived => "archived",
            MoveState::Expired => "expired",
            MoveState::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(s: &str) -> Option<MoveState> {
        MoveState::ALL.into_iter().find(|m| m.as_str() == s)
    }

    pub const ALL: [MoveState; 14] = [
        MoveState::Listed,
        MoveState::Open,
        MoveState::Later,
        MoveState::Never,
        MoveState::Asked,
        MoveState::Staging,
        MoveState::Importing,
        MoveState::Retiring,
        MoveState::Moved,
        MoveState::Held,
        MoveState::Archiving,
        MoveState::Archived,
        MoveState::Expired,
        MoveState::Withdrawn,
    ];
}

/// Why a move is held for staff.
///
/// Open (`x-fragment-open`, docs/wire): a reader shows a reason it does
/// not know as it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(extend("x-fragment-open" = true)))]
#[serde(rename_all = "snake_case")]
pub enum HeldReason {
    /// The mover never said it stopped the old agent.
    MoverDown,
    /// Staging failed, or stalled, `ATTEMPTS_MAX` times.
    StageFailed,
    /// The new agent never opened its stage.
    NotOpened,
    /// The new agent said its import failed.
    ImportFailed,
    /// The new agent never said it was done.
    ImportTimedOut,
    /// The mover never said it retired the old agent.
    RetireTimedOut,
    /// Phase 2's stage or retirement failed.
    ArchiveFailed,
}

impl HeldReason {
    pub fn as_str(self) -> &'static str {
        match self {
            HeldReason::MoverDown => "mover_down",
            HeldReason::StageFailed => "stage_failed",
            HeldReason::NotOpened => "not_opened",
            HeldReason::ImportFailed => "import_failed",
            HeldReason::ImportTimedOut => "import_timed_out",
            HeldReason::RetireTimedOut => "retire_timed_out",
            HeldReason::ArchiveFailed => "archive_failed",
        }
    }

    pub fn parse(s: &str) -> Option<HeldReason> {
        [
            HeldReason::MoverDown,
            HeldReason::StageFailed,
            HeldReason::NotOpened,
            HeldReason::ImportFailed,
            HeldReason::ImportTimedOut,
            HeldReason::RetireTimedOut,
            HeldReason::ArchiveFailed,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }
}

// ---- The mover's routes (`/api/migrations/mover/…`)

/// `PUT /api/migrations/mover/roster`: every live old agent and its
/// owner, whole. A snapshot replaces the last: a row the person has not
/// answered Yes to that is missing from it is withdrawn, and a row in
/// flight is left as it is. The same `snapshot` again changes nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Roster {
    /// The mover's id for this snapshot (`TEXT_MAX_BYTES`).
    pub snapshot: String,
    /// When the mover read the old platform, ms since the epoch.
    pub observed_ms: i64,
    /// At most `ROSTER_PEOPLE_MAX`, each WorkOS id once.
    pub people: Vec<RosterPerson>,
}

/// A person on the roster: their WorkOS user (the one both platforms sign
/// them in as) and their live old agents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RosterPerson {
    pub workos_id: String,
    /// Their email on the old platform: where phase 2's mail goes.
    pub email: String,
    /// At most `ROSTER_AGENTS_MAX`, each old id once in the roster.
    pub agents: Vec<RosterAgent>,
}

/// An old agent as the roster lists it: what the person is asked about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RosterAgent {
    /// Its id (`valid_old`).
    pub old: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub picture_url: Option<String>,
    /// When it was last used, ms since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub last_active_ms: Option<i64>,
    pub sizes: Sizes,
    /// Brains its person owns (`ROSTER_LIST_MAX`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub brains: Vec<OldBrain>,
    /// Sites it owns (`ROSTER_LIST_MAX`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub sites: Vec<OldSite>,
    pub has: Has,
}

/// An old agent's bytes by class, as the one classifier counts them (the
/// staged class is `move`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Sizes {
    pub cache: u64,
    pub history: u64,
    pub work: u64,
    /// What is staged: history, work, owned brains and sites.
    #[serde(rename = "move")]
    pub staged: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OldBrain {
    pub id: String,
    pub title: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OldSite {
    pub name: String,
    pub public: bool,
}

/// Which secrets it holds (never their values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Has {
    pub telegram: bool,
    pub openrouter: bool,
    pub nostr: bool,
}

/// The roster's answer: the rows it offered at once (new rows of a person
/// bound here already: a person is bound when they open the move), the
/// rows it withdrew, and whether it was the last snapshot again (then
/// nothing changed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RosterTaken {
    pub bound: u32,
    pub withdrawn: u32,
    pub replay: bool,
}

/// `GET /api/migrations/mover/mailbox`: what the mover is told to do,
/// oldest first, at most `MAILBOX_MAX`. A command stays until a report on
/// it is taken, so reading again is the retry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Mailbox {
    pub commands: Vec<Command>,
}

/// One thing to do to one old agent, flat: its `kind`, and the fields
/// that kind has. Every step is idempotent on the mover's side: a stop of
/// a stopped runtime does nothing, and a stage uploads the same parts again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Command {
    /// `command_id`: `<old>:<attempt>:<step>`.
    pub id: String,
    pub old: String,
    pub attempt: u32,
    pub kind: CommandKind,
    /// `stop_and_stage`: its phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub phase: Option<Phase>,
    /// `stop_and_stage`: the stage to upload (`stage_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub stage: Option<String>,
    /// `retire`: how the old agent is marked moved.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub mark: Option<Mark>,
    /// `stop_and_stage`: the secrets to hand on (none until the vault: FIN-466).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub secrets: Vec<SecretKind>,
}

/// What a command asks.
///
/// Open (`x-fragment-open`, docs/wire): the mover leaves a command it
/// does not know in the mailbox, done by no one, and the move is held
/// when its deadline passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(extend("x-fragment-open" = true)))]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    /// Stop the old agent, then upload its zip as `stage`, in parts, and
    /// complete it (the manifest last).
    StopAndStage,
    /// Retire it (a verified backup first) and mark it moved, `as`.
    Retire,
    /// Start it again: staff gave it back (`restart-old`).
    Restart,
}

impl CommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandKind::StopAndStage => "stop_and_stage",
            CommandKind::Retire => "retire",
            CommandKind::Restart => "restart",
        }
    }
}

/// How a retired old agent is marked on the old platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Its person brought it over.
    Migrated,
    /// Phase 2: staged and turned off at the cutoff.
    Archived,
}

/// `POST /api/migrations/mover/agents/{old}/report`: what happened on the
/// command it names. Taken only for that old agent's current command;
/// another (a stale attempt's) is 409, changing nothing. `seq` only grows
/// per command: one taken already is a 200 that changes nothing.
///
/// Flat, one shape for every kind, read as plain fields: its `kind`, and
/// the fields that kind has. `stopped` (the old agent is a folder now);
/// `progress`, `bytes` and `files` so far; `staged` (`POST …/complete`
/// done), `manifest_sha256`, `bytes` and `files` in all; `failed` (the
/// platform decides whether to try again), `step` and `why`; `retired`
/// (and marked moved), `receipt`; `restarted`. Another kind, or a field a
/// kind needs and lacks, is 400.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Report {
    pub command: String,
    pub seq: u64,
    pub kind: String,
    /// Bytes and files staged so far (`progress`), or in all (`staged`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub files: Option<u64>,
    /// `staged`: the manifest's SHA-256 (hex), as `POST …/complete` answered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub manifest_sha256: Option<String>,
    /// `failed`: the step, and why (`TEXT_MAX_BYTES` each).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub why: Option<String>,
    /// `retired`: Core's retirement snapshot's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub receipt: Option<String>,
}

impl Report {
    /// A report of `kind` alone, its fields to fill.
    pub fn of(command: &str, seq: u64, kind: &str) -> Report {
        Report { command: command.into(), seq, kind: kind.into(), bytes: None, files: None, manifest_sha256: None, step: None, why: None, receipt: None }
    }
}

/// A report's answer: the move's state after it, its attempt, and
/// whether this report changed anything (false: its `seq` was taken).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Reported {
    pub state: MoveState,
    pub attempt: u32,
    pub taken: bool,
}

/// A secret an old agent holds that its person may bring (FIN-466's vault;
/// not handed on yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    TelegramBotToken,
    OpenrouterKey,
    /// The old agent's Nostr key: it signs for the old platform's services alone.
    NostrNsec,
}

/// `POST /api/migrations/mover/agents/{old}/secrets` (F11, once the vault
/// lands; no route yet): the values the person said yes to, forwarded to
/// the vault within the call and never stored, logged or echoed here.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Handoff {
    pub command: String,
    pub items: Vec<HandoffItem>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoffItem {
    pub kind: SecretKind,
    pub value: String,
}

/// Never shows a value.
impl std::fmt::Debug for Handoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kinds: Vec<SecretKind> = self.items.iter().map(|i| i.kind).collect();
        f.debug_struct("Handoff").field("command", &self.command).field("kinds", &kinds).finish()
    }
}

impl std::fmt::Debug for HandoffItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandoffItem").field("kind", &self.kind).finish_non_exhaustive()
    }
}

/// Its answer: the kinds handed to the vault.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandedOff {
    pub handed: Vec<SecretKind>,
}

// ---- The mover's upload (`/api/migrations/mover/stages/{stage}/…`), for the
// stage its old agent's current `stop_and_stage` names, while it stages. The
// old platform holds no bucket credential: R2 has no write-only one.

/// One part taken (`PUT …/parts/{n}`), and each part `GET …/parts` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StagePart {
    pub n: u32,
    pub bytes: u64,
    /// Its SHA-256 (hex), as the mover's signed URL named it: kept for the
    /// mover's resume. The bytes stream to R2 unread; the zip's own SHA-256
    /// in the manifest is what the importer checks.
    pub sha256: String,
    /// R2's etag for it.
    pub etag: String,
}

/// `GET …/parts`: the parts taken so far, by number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StageParts {
    pub parts: Vec<StagePart>,
}

/// `POST …/complete`: the stage's manifest, for its zip made whole from
/// the parts taken (their bytes in all its `zip_bytes`; R2 refuses a part
/// but the last under `PART_MIN_BYTES`), the manifest written last. The
/// same complete again answers the same; one cut off between the zip and
/// its manifest is not made again (its attempt stalls, and the next stages
/// afresh).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StageComplete {
    pub manifest: Manifest,
}

/// The manifest as written, by its SHA-256 (hex): what the mover's
/// `staged` report names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StageCompleted {
    pub manifest_sha256: String,
    pub zip_bytes: u64,
}

// ---- The new agent's routes (`/api/migrations/stages/{stage}/…`), each
// for the agent the move is bound to, while it imports

/// `manifest.json`, as the mover writes it beside the zip (last).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Manifest {
    /// `STAGE_FORMAT`.
    pub format: String,
    pub old: String,
    pub phase: Phase,
    pub attempt: u32,
    /// How the folder is laid out, for the importer: `finite-computer`.
    pub layout: String,
    /// The zip's size and SHA-256 (hex).
    pub zip_bytes: u64,
    pub zip_sha256: String,
    /// The files inside it, uncompressed.
    pub bytes: u64,
    pub files: u64,
    /// What stayed behind, and why (`MANIFEST_LEFT_MAX` of them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub left: Vec<LeftBehind>,
    /// Paths left behind past the list, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub left_more: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LeftBehind {
    pub path: String,
    pub bytes: u64,
    /// `cache`, `runtime`, `secret`, … (the classifier's class).
    pub why: String,
}

/// `POST /api/migrations/stages/{stage}/open`: the stage's manifest. Then
/// `GET …/zip` reads the zip by ranges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StageOpened {
    pub stage: String,
    pub manifest: Manifest,
}

/// `POST /api/migrations/stages/{stage}/done`: the import ended, well or
/// not. The same `ok` again changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StageDone {
    pub ok: bool,
    /// The SHA-256 (hex) of the report the agent wrote its owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub report_sha256: Option<String>,
    /// Why it failed (`TEXT_MAX_BYTES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub why: Option<String>,
}

/// The move's state, after a call that may have changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MoveAt {
    pub state: MoveState,
}

// ---- The operators' routes (`/api/admin/migrations`; not on the wire)

/// `POST /api/admin/migrations/{old}`: staff's act on one move: `op`
/// `retry`, `restart-old`, `retire` or `withdraw` (another is 400).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminAct {
    pub op: String,
}

/// `GET /api/admin/migrations[?after=<old>]`: every move, by old id, a
/// page at a time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminMoves {
    pub moves: Vec<AdminMove>,
    /// The old id to read after, when there are more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// One move as staff see it: ids and state, no name or email.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminMove {
    pub old: String,
    pub workos_id: String,
    /// Its person here, once bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// The new agent it is bound to, after Yes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub state: MoveState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<HeldReason>,
    pub phase: Phase,
    pub attempt: u32,
    /// Its current command's id, while the mailbox holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// When it entered its state, and when the supervisor next looks at it.
    pub since_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<i64>,
    /// When its stage was written, while it is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A stage's id names its old agent, phase and attempt, and reads back
    /// as them; anything else reads as none.
    #[test]
    fn a_stage_id_reads_back() {
        let id = stage_id(Phase::P1, "prj_1-a", 2);
        assert_eq!(id, "prj_1-a.p1.2");
        assert_eq!(parse_stage(&id), Some(("prj_1-a".into(), Phase::P1, 2)));
        assert_eq!(stage_prefix(Phase::P2, "prj_1-a", 1), "p2/prj_1-a/1/");
        for bad in ["", "a.p1", "a.p3.1", "a.p1.0", "a.p1.01", "a.p1.x", ".p1.1", "a/b.p1.1", "a.b.p1.1", &format!("{}.p1.1", "a".repeat(65))] {
            assert_eq!(parse_stage(bad), None, "{bad:?}");
        }
        assert!(valid_old(&"a".repeat(64)) && !valid_old("a b") && !valid_old("../a"));
    }

    /// The mailbox on the wire, flat: its kind and that kind's fields, `as`
    /// a retirement's mark.
    #[test]
    fn a_command_on_the_wire() {
        let c = Command { id: command_id("o", 1, "stop_and_stage"), old: "o".into(), attempt: 1, kind: CommandKind::StopAndStage, phase: Some(Phase::P1), stage: Some(stage_id(Phase::P1, "o", 1)), mark: None, secrets: vec![] };
        assert_eq!(serde_json::to_value(&c).unwrap(), json!({ "id": "o:1:stop_and_stage", "old": "o", "attempt": 1, "kind": "stop_and_stage", "phase": "p1", "stage": "o.p1.1" }));
        let retire = Command { id: command_id("o", 1, "retire_migrated"), old: "o".into(), attempt: 1, kind: CommandKind::Retire, phase: None, stage: None, mark: Some(Mark::Migrated), secrets: vec![] };
        let v = serde_json::to_value(&retire).unwrap();
        assert_eq!(v, json!({ "id": "o:1:retire_migrated", "old": "o", "attempt": 1, "kind": "retire", "as": "migrated" }));
        assert_eq!(serde_json::from_value::<Command>(v).unwrap(), retire);
    }

    /// A report's events, as the mover sends them.
    #[test]
    fn a_report_on_the_wire() {
        let r: Report = serde_json::from_value(json!({ "command": "o:1:stop_and_stage", "seq": 3, "kind": "staged", "manifest_sha256": "ab", "bytes": 10, "files": 2 })).unwrap();
        assert_eq!((r.kind.as_str(), r.manifest_sha256.as_deref(), r.bytes, r.files), ("staged", Some("ab"), Some(10), Some(2)));
        let stopped: Report = serde_json::from_value(json!({ "command": "c", "seq": 1, "kind": "stopped" })).unwrap();
        assert_eq!(stopped, Report::of("c", 1, "stopped"));
        assert_eq!(serde_json::to_value(&stopped).unwrap(), json!({ "command": "c", "seq": 1, "kind": "stopped" }));
        // a kind is read as text: one there is not is the platform's to refuse
        assert!(serde_json::from_value::<Report>(json!({ "command": "c", "seq": 1 })).is_err());
    }

    /// A roster row with what it may leave out, and `move` its staged bytes.
    #[test]
    fn a_roster_row_on_the_wire() {
        let a: RosterAgent = serde_json::from_value(json!({ "old": "o", "name": "Ada", "sizes": { "cache": 1, "history": 2, "work": 3, "move": 5, "total": 6 }, "has": { "telegram": true, "openrouter": false, "nostr": true } })).unwrap();
        assert_eq!(a.sizes.staged, 5);
        assert!(a.brains.is_empty() && a.sites.is_empty() && a.picture_url.is_none());
    }

    /// A hand-off's values never show in its `Debug`.
    #[test]
    fn a_handoff_never_shows_a_value() {
        let h = Handoff { command: "c".into(), items: vec![HandoffItem { kind: SecretKind::TelegramBotToken, value: "123:SECRET".into() }] };
        let shown = format!("{h:?} {:?}", h.items[0]);
        assert!(!shown.contains("SECRET") && shown.contains("TelegramBotToken"), "{shown}");
    }

    /// Each state and reason reads back from its name.
    #[test]
    fn states_and_reasons_read_back() {
        for s in MoveState::ALL {
            assert_eq!(MoveState::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        assert_eq!(serde_json::to_value(HeldReason::NotOpened).unwrap(), json!(HeldReason::NotOpened.as_str()));
        assert_eq!(HeldReason::parse("import_failed"), Some(HeldReason::ImportFailed));
    }
}
