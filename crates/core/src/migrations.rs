//! One old agent's move, as a pure state machine (docs/migrations.md,
//! "The state machine"): the `Migrations` object (cell/src/migrations.rs)
//! holds a `Move` per old agent and asks this for every change, with the
//! clock passed in. Each input answers the move after it and what to do
//! besides (`Effect`), or `None` when it changes nothing (a replay), or a
//! refusal that changes nothing either.
//!
//! The mailbox is the moves whose `command` is set: one command per old
//! agent at most, replaced as its move goes on, so a report on any other
//! is stale. Every deadline is a function of the move alone (`deadline`),
//! so a restarted object, or one whose alarm slept, reads the same. A
//! move is stored as plain columns (`COLUMNS`, `Move::read`,
//! `Move::values`): no reader of its own in the cell's Wasm.
//! Removable with the move (docs/technical-debt-ledger.md).

use fragment_proto::migrations::{
    command_id, stage_id, stage_prefix, Command, CommandKind, Handoff, Has, HeldReason, Mark, MoveState, Phase, Report, SecretKind, StageDone, ATTEMPTS_MAX, HANDOFF_VALUE_MAX_BYTES, TEXT_MAX_BYTES, WHY_MAX_BYTES,
};
use fragment_proto::vault::{Place, PutSecret};
use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel};
use serde_json::Value;

const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// "Not now" asks again after this.
pub const LATER_MS: i64 = DAY_MS;
/// After Yes, the mover says it stopped the old agent within this, or the
/// move is held (the mover is down).
pub const STOP_MS: i64 = HOUR_MS;
/// Staging says how it goes at least this often...
pub const STALL_MS: i64 = 30 * MINUTE_MS;
/// ...and is done within this (the largest old agent, 100 GB at 100 MB/s,
/// takes about 17 minutes).
pub const STAGE_MS: i64 = 12 * HOUR_MS;
/// The new agent opens its stage within this of its routine (a routine
/// read more than an hour late is skipped: docs/chat-records.md).
pub const OPEN_MS: i64 = 2 * HOUR_MS;
/// Routines appended for one stage before the move is held unopened.
pub const ROUTINES_MAX: u32 = 2;
/// The new agent says it is done within this of its stage being written.
pub const DONE_MS: i64 = DAY_MS;
/// The mover says it retired the old agent within this.
pub const RETIRE_MS: i64 = DAY_MS;
/// A phase 1 stage is kept this long after it is written, done or not
/// (the old host's encrypted backup remains).
pub const KEEP_MS: i64 = 14 * DAY_MS;
/// A phase 2 stage is kept this long after the cutoff, for a late Yes.
pub const ARCHIVE_KEEP_MS: i64 = 90 * DAY_MS;
/// Phase 2 moves in flight at once: the rest wait their turn.
pub const ARCHIVING_MAX: usize = 8;
/// A person on the roster's comp (F5), given once as they open the move:
/// a seat for this long...
pub const COMP_MS: i64 = 30 * DAY_MS;
/// ...and this much credit, granted on their ledger once, by its id
/// (`comp_grant_id`).
pub const COMP_GRANT_MICROS: i64 = 25 * crate::price::USD;

/// The comp's grant's id on `person`'s ledger: one a person, ever (a
/// ledger answers the same id again as it did, granting nothing).
pub fn comp_grant_id(person: &str) -> String {
    format!("move:{person}")
}

/// The routine that starts a stage's import, on its new agent's own
/// `tasks` with no chat, so it runs in its owner's home (docs/migrations.md;
/// docs/chat-records.md, "A routine"). Its person's answers join it with
/// the page that asks them (F6).
pub fn routine_text(stage: &str) -> String {
    format!("Bring yourself over from finite.computer: use the import-agent-home skill, layout finite-computer, stage {stage}.")
}

/// What the mailbox holds for a move. A stage's phase and id are the
/// move's own (`Move::stage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    StopAndStage,
    Retire(Mark),
    Restart,
}

impl Act {
    pub fn kind(self) -> CommandKind {
        match self {
            Act::StopAndStage => CommandKind::StopAndStage,
            Act::Retire(_) => CommandKind::Retire,
            Act::Restart => CommandKind::Restart,
        }
    }

    /// As its column holds it, and its command's id names it (its step:
    /// a retirement's with its mark, so phase 2's two are two commands).
    fn stored(self) -> &'static str {
        match self {
            Act::StopAndStage => "stop_and_stage",
            Act::Retire(Mark::Migrated) => "retire_migrated",
            Act::Retire(Mark::Archived) => "retire_archived",
            Act::Restart => "restart",
        }
    }

    fn parse(s: &str) -> Option<Act> {
        [Act::StopAndStage, Act::Retire(Mark::Migrated), Act::Retire(Mark::Archived), Act::Restart].into_iter().find(|a| a.stored() == s)
    }
}

/// One old agent's move, as the object stores it (`COLUMNS`: a column
/// added later reads from an older row as its default).
#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    pub old: String,
    pub workos_id: String,
    pub state: MoveState,
    /// A held move's reason.
    pub reason: Option<HeldReason>,
    /// The state a held move was held from: what a retry goes back to.
    pub held_from: Option<MoveState>,
    pub phase: Phase,
    /// 0 until the first Yes (or cutoff); each stage is one attempt.
    pub attempt: u32,
    /// The new agent (its identity) after Yes.
    pub agent: Option<String>,
    /// The mailbox's command for it, if any, and the last `seq` taken on it.
    pub command: Option<Act>,
    pub seq: u64,
    /// When it entered its state.
    pub since_ms: i64,
    /// Staging: the mover's last word (stopped or progress). Archiving's
    /// retirement: when its stage was written.
    pub progress_ms: Option<i64>,
    /// When its attempt's stage was written, until it is swept.
    pub staged_ms: Option<i64>,
    /// Importing: when its routine was last appended, how many were, and
    /// whether the agent opened its stage.
    pub routine_ms: Option<i64>,
    pub routines: u32,
    pub opened: bool,
    /// Its imports so far, one more at each (`importing`), never reset:
    /// its routines' keys name it, so a retry's never repeat an earlier
    /// import's (`routine_key`).
    pub generation: u32,
    /// Phase 2: when its stage expires (the cutoff, plus `ARCHIVE_KEEP_MS`).
    pub expires_ms: Option<i64>,
    /// Withdrawn by staff: a roster that lists it again leaves it so.
    pub by_staff: bool,
    /// Why staff withdrew it, while they keep it out (`withdraw`).
    pub why: Option<String>,
    /// Missing from the last roster (or listed by none yet): staff's
    /// withdrawal taken back leaves it withdrawn, the roster's (`include`).
    pub dropped: bool,
}

/// A move's columns in the object's `moves` table, in order: the last,
/// its deadline, derived from the rest (`Move::deadline`), for the alarm
/// to find it by.
pub const COLUMNS: [&str; 22] = [
    "old", "workos_id", "state", "reason", "held_from", "phase", "attempt", "agent", "command", "seq", "since_ms", "progress_ms", "staged_ms", "routine_ms", "routines", "opened", "expires_ms", "by_staff", "generation", "why", "dropped", "deadline_ms",
];

/// A column's value, as SQLite takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Col {
    Null,
    Int(i64),
    Text(String),
}

impl Move {
    /// Its columns' values, `COLUMNS`' order.
    pub fn values(&self) -> Vec<Col> {
        let text = |s: &str| Col::Text(s.to_string());
        let opt_text = |s: Option<&str>| s.map_or(Col::Null, text);
        let opt_int = |n: Option<i64>| n.map_or(Col::Null, Col::Int);
        vec![
            text(&self.old),
            text(&self.workos_id),
            text(self.state.as_str()),
            opt_text(self.reason.map(HeldReason::as_str)),
            opt_text(self.held_from.map(MoveState::as_str)),
            text(self.phase.as_str()),
            Col::Int(i64::from(self.attempt)),
            opt_text(self.agent.as_deref()),
            opt_text(self.command.map(Act::stored)),
            Col::Int(self.seq as i64),
            Col::Int(self.since_ms),
            opt_int(self.progress_ms),
            opt_int(self.staged_ms),
            opt_int(self.routine_ms),
            Col::Int(i64::from(self.routines)),
            Col::Int(i64::from(self.opened)),
            opt_int(self.expires_ms),
            Col::Int(i64::from(self.by_staff)),
            Col::Int(i64::from(self.generation)),
            opt_text(self.why.as_deref()),
            Col::Int(i64::from(self.dropped)),
            opt_int(self.deadline()),
        ]
    }

    /// A move from its row, each column by name as SQLite answered it; a
    /// column missing reads as its default.
    pub fn read(row: &Value) -> Result<Move, String> {
        let text = |c: &str| row.get(c).and_then(Value::as_str);
        let int = |c: &str| row.get(c).and_then(Value::as_i64);
        let need = |c: &str| text(c).ok_or_else(|| format!("a move's {c} is missing"));
        let bad = |c: &str| format!("a move's {c} is no value it takes");
        Ok(Move {
            old: need("old")?.into(),
            workos_id: need("workos_id")?.into(),
            state: MoveState::parse(need("state")?).ok_or_else(|| bad("state"))?,
            reason: text("reason").and_then(HeldReason::parse),
            held_from: text("held_from").and_then(MoveState::parse),
            phase: match need("phase")? {
                "p1" => Phase::P1,
                "p2" => Phase::P2,
                _ => return Err(bad("phase")),
            },
            attempt: int("attempt").and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
            agent: text("agent").map(str::to_string),
            command: match text("command") {
                None => None,
                Some(c) => Some(Act::parse(c).ok_or_else(|| bad("command"))?),
            },
            seq: int("seq").and_then(|n| u64::try_from(n).ok()).unwrap_or(0),
            since_ms: int("since_ms").ok_or_else(|| bad("since_ms"))?,
            progress_ms: int("progress_ms"),
            staged_ms: int("staged_ms"),
            routine_ms: int("routine_ms"),
            routines: int("routines").and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
            opened: int("opened") == Some(1),
            expires_ms: int("expires_ms"),
            by_staff: int("by_staff") == Some(1),
            generation: int("generation").and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
            why: text("why").map(str::to_string),
            dropped: int("dropped") == Some(1),
        })
    }
}

/// What to do besides storing the move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Append the import's routine to the new agent's own fragment, for
    /// this stage (`stage_id`).
    Routine { agent: String, stage: String },
    /// Delete this stage's objects (`stage_prefix`): its routes already
    /// refuse, as the move no longer names it.
    Sweep { prefix: String },
    /// Delete its row: staff withdrew it ahead of any roster, and took that
    /// back (`include`).
    Forget,
}

/// A change: the move after it, and its effects.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub to: Move,
    pub effects: Vec<Effect>,
}

/// Why an input is refused; nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not in this state (409).
    NotNow(MoveState),
    /// A report on a command that is not its current one (409).
    Stale,
    /// Not the agent the move is bound to (403).
    NotYours,
    /// Not its current stage (404).
    NoStage,
    /// An event that does not fit its command (400).
    Unfit(&'static str),
    /// A withdrawal's why, empty or over `WHY_MAX_BYTES` (400).
    Why,
    /// Its old agent may be stopped or retired: a person's Yes, or the
    /// cutoff, began its move (409).
    Begun,
}

impl Refused {
    pub fn message(&self) -> String {
        match self {
            Refused::NotNow(s) => format!("not while the move is {}", s.as_str()),
            Refused::Stale => "not this old agent's current command: a stale attempt's, or done".into(),
            Refused::NotYours => "only the agent this move is bound to".into(),
            Refused::NoStage => "no such stage: another attempt's, or none".into(),
            Refused::Unfit(why) => (*why).into(),
            Refused::Why => format!("a withdrawal says why, in 1 to {WHY_MAX_BYTES} bytes"),
            Refused::Begun => "its move began (its person said Yes, or the cutoff took it): its old agent may be stopped or retired, so it is not offered again; bring it over by hand".into(),
        }
    }
}

/// An input's answer: a change, nothing (a replay), or a refusal.
pub type Outcome = Result<Option<Step>, Refused>;

impl Move {
    /// A row the roster lists for the first time.
    pub fn listed(old: &str, workos_id: &str, bound: bool, now: i64) -> Move {
        Move {
            old: old.into(),
            workos_id: workos_id.into(),
            state: if bound { MoveState::Open } else { MoveState::Listed },
            reason: None,
            held_from: None,
            phase: Phase::P1,
            attempt: 0,
            agent: None,
            command: None,
            seq: 0,
            since_ms: now,
            progress_ms: None,
            staged_ms: None,
            routine_ms: None,
            routines: 0,
            opened: false,
            expires_ms: None,
            by_staff: false,
            generation: 0,
            why: None,
            dropped: false,
        }
    }

    /// A row for an old agent no roster has listed yet, for staff to
    /// withdraw ahead of it (`withdraw`): no person, and missing from the
    /// roster. A roster that lists it fills in its person (`relisted`).
    pub fn unlisted(old: &str, now: i64) -> Move {
        Move { state: MoveState::Withdrawn, dropped: true, ..Move::listed(old, "", false, now) }
    }

    /// Whether a roster ever listed it: its person is the roster's.
    pub fn rostered(&self) -> bool {
        !self.workos_id.is_empty()
    }

    /// Its current command's id, while it has one.
    pub fn command_id(&self) -> Option<String> {
        self.command.map(|act| command_id(&self.old, self.attempt, act.stored()))
    }

    /// Its command as the mailbox answers it, flat.
    pub fn command(&self) -> Option<Command> {
        let act = self.command?;
        let staging = act == Act::StopAndStage;
        Some(Command {
            id: command_id(&self.old, self.attempt, act.stored()),
            old: self.old.clone(),
            attempt: self.attempt,
            kind: act.kind(),
            phase: staging.then_some(self.phase),
            stage: if staging { self.stage() } else { None },
            mark: match act {
                Act::Retire(mark) => Some(mark),
                _ => None,
            },
            secrets: vec![],
        })
    }

    /// The secrets its current command hands to its person's Vault: on a
    /// person's Yes (a phase 1 `stop_and_stage`, bound to their agent),
    /// every kind its old agent has, with no question of its own (the
    /// Vault is theirs, and nothing uses an entry until they set
    /// something up); none otherwise.
    pub fn secrets(&self, has: &Has) -> Vec<SecretKind> {
        if self.command != Some(Act::StopAndStage) || self.phase != Phase::P1 || self.agent.is_none() {
            return vec![];
        }
        let held = [has.telegram, has.openrouter, has.nostr];
        SecretKind::ALL.into_iter().zip(held).filter_map(|(k, h)| h.then_some(k)).collect()
    }

    /// Its current attempt's stage id and prefix (none before an attempt).
    pub fn stage(&self) -> Option<String> {
        (self.attempt >= 1).then(|| stage_id(self.phase, &self.old, self.attempt))
    }

    fn prefix(&self) -> String {
        stage_prefix(self.phase, &self.old, self.attempt)
    }

    /// Its import's routine's key, as the agent's fragment appends it once:
    /// the same for each try of one delivery, and new for each routine of
    /// each import (`generation`), a retry's included.
    pub fn routine_key(&self) -> Option<String> {
        self.stage().map(|stage| format!("migration:{stage}:{}.{}", self.generation, self.routines))
    }

    /// Whether its person may still be asked (or the cutoff take it).
    pub fn unasked(&self) -> bool {
        matches!(self.state, MoveState::Listed | MoveState::Open | MoveState::Later | MoveState::Never)
    }

    /// When the supervisor next looks at it, if ever: its state's
    /// deadline, or its stage's keep.
    pub fn deadline(&self) -> Option<i64> {
        let state = match self.state {
            MoveState::Later => Some(self.since_ms + LATER_MS),
            MoveState::Asked => Some(self.since_ms + STOP_MS),
            MoveState::Staging => Some(stall(self)),
            MoveState::Importing => Some(match (self.opened, self.routine_ms) {
                (false, Some(at)) => at + OPEN_MS,
                _ => self.since_ms + DONE_MS,
            }),
            MoveState::Retiring => Some(self.since_ms + RETIRE_MS),
            MoveState::Archiving => Some(match &self.command {
                Some(Act::Retire(_)) => self.progress_ms.unwrap_or(self.since_ms) + RETIRE_MS,
                _ if self.progress_ms.is_none() => self.since_ms + STOP_MS,
                _ => stall(self),
            }),
            MoveState::Archived => self.expires_ms,
            _ => None,
        };
        match (state, self.stage_expires()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// When its stage goes, whatever else it waits for: a phase 1 one
    /// `KEEP_MS` after it was written, done or not; a phase 2 one at its
    /// expiry, however its move ended (it is the only copy left: its old
    /// agent was retired as archived). An archived one goes as it expires
    /// (`tick`), and one being imported is read until its import ends.
    fn stage_expires(&self) -> Option<i64> {
        let at = self.staged_ms?;
        match (self.phase, self.state) {
            (Phase::P1, _) => Some(at + KEEP_MS),
            (Phase::P2, MoveState::Archived | MoveState::Importing) => None,
            (Phase::P2, _) => self.expires_ms,
        }
    }

    // ---- helpers that make the next move

    fn moved(&self, state: MoveState, now: i64) -> Move {
        Move { state, since_ms: now, reason: None, held_from: None, ..self.clone() }
    }

    /// Held for staff, from its state now: no command left.
    fn held(&self, reason: HeldReason, now: i64) -> Move {
        Move { state: MoveState::Held, reason: Some(reason), held_from: Some(self.state), command: None, seq: 0, since_ms: now, ..self.clone() }
    }

    /// Its current stage, swept: the effect, and the move without it. A
    /// written phase 2 stage is kept: it goes at its expiry alone
    /// (`expired`).
    fn swept(self, effects: &mut Vec<Effect>) -> Move {
        match self.phase == Phase::P2 && self.staged_ms.is_some() {
            true => self,
            false => self.expired(effects),
        }
    }

    /// Its current stage, swept whatever its phase: past its keep.
    fn expired(mut self, effects: &mut Vec<Effect>) -> Move {
        if self.attempt >= 1 {
            effects.push(Effect::Sweep { prefix: self.prefix() });
        }
        self.staged_ms = None;
        self
    }

    /// A new attempt at its stage, in `phase`: the mover told to stop and
    /// upload it as the new attempt's stage.
    fn attempt_again(&self, state: MoveState, phase: Phase, now: i64) -> Move {
        let attempt = self.attempt + 1;
        Move {
            state,
            reason: None,
            held_from: None,
            phase,
            attempt,
            command: Some(Act::StopAndStage),
            seq: 0,
            since_ms: now,
            progress_ms: None,
            staged_ms: None,
            routine_ms: None,
            routines: 0,
            opened: false,
            ..self.clone()
        }
    }

    /// Staged: its new agent is told to bring itself over.
    fn importing(&self, agent: String, now: i64, staged_ms: i64) -> Step {
        let to = Move {
            state: MoveState::Importing,
            reason: None,
            held_from: None,
            agent: Some(agent.clone()),
            command: None,
            seq: 0,
            since_ms: now,
            progress_ms: None,
            staged_ms: Some(staged_ms),
            routine_ms: Some(now),
            routines: 1,
            opened: false,
            generation: self.generation + 1,
            ..self.clone()
        };
        let stage = to.stage().expect("an import is of an attempt");
        Step { to, effects: vec![Effect::Routine { agent, stage }] }
    }

    /// A stage that failed or stalled: tried again as a new attempt, up to
    /// `ATTEMPTS_MAX`, else held; the half-written one swept either way.
    fn stage_again_or_hold(&self, reason: HeldReason, now: i64) -> Step {
        let mut effects = vec![];
        let from = self.clone().swept(&mut effects);
        let to = match self.attempt < ATTEMPTS_MAX {
            true => from.attempt_again(if self.phase == Phase::P2 { MoveState::Archiving } else { MoveState::Asked }, self.phase, now),
            false => from.held(reason, now),
        };
        Step { to, effects }
    }
}

/// A staging's deadline: its last word plus `STALL_MS`, within `STAGE_MS`
/// of its start.
fn stall(m: &Move) -> i64 {
    (m.progress_ms.unwrap_or(m.since_ms) + STALL_MS).min(m.since_ms + STAGE_MS)
}

fn step(to: Move) -> Outcome {
    Ok(Some(Step { to, effects: vec![] }))
}

// ---- the roster

/// Its person became known here: listed → open.
pub fn bind(m: &Move, now: i64) -> Option<Step> {
    (m.state == MoveState::Listed).then(|| Step { to: m.moved(MoveState::Open, now), effects: vec![] })
}

/// Missing from a roster: withdrawn, if its person never said Yes. A
/// command it holds (a restart) stays. One staff withdrew stays theirs,
/// marked dropped (`include` leaves it withdrawn).
pub fn withdrawn(m: &Move, now: i64) -> Option<Step> {
    let to = match m.state {
        _ if m.unasked() => m.moved(MoveState::Withdrawn, now),
        MoveState::Withdrawn if m.by_staff && !m.dropped => m.clone(),
        _ => return None,
    };
    Some(Step { to: Move { dropped: true, ..to }, effects: vec![] })
}

/// Listed again (by `workos_id`'s roster row) after a roster withdrew it:
/// offered again. One staff withdrew stays withdrawn, no longer dropped,
/// and one withdrawn ahead of any roster takes its person from it.
pub fn relisted(m: &Move, workos_id: &str, bound: bool, now: i64) -> Option<Step> {
    let to = match (m.state, m.by_staff) {
        (MoveState::Withdrawn, false) => m.moved(if bound { MoveState::Open } else { MoveState::Listed }, now),
        (MoveState::Withdrawn, true) if m.dropped => m.clone(),
        _ => return None,
    };
    let workos_id = if m.rostered() { m.workos_id.clone() } else { workos_id.to_string() };
    Some(Step { to: Move { workos_id, dropped: false, ..to }, effects: vec![] })
}

// ---- its person's answers

/// "Not now": asked again after `LATER_MS`.
pub fn later(m: &Move, now: i64) -> Outcome {
    match m.state {
        MoveState::Open => step(m.moved(MoveState::Later, now)),
        MoveState::Later => Ok(None),
        s => Err(Refused::NotNow(s)),
    }
}

/// "Never": not asked, until they ask again.
pub fn never(m: &Move, now: i64) -> Outcome {
    match m.state {
        MoveState::Open | MoveState::Later => step(m.moved(MoveState::Never, now)),
        MoveState::Never => Ok(None),
        s => Err(Refused::NotNow(s)),
    }
}

/// "Ask again", from `/move`: offered again.
pub fn ask_again(m: &Move, now: i64) -> Outcome {
    match m.state {
        MoveState::Never => step(m.moved(MoveState::Open, now)),
        MoveState::Open => Ok(None),
        s => Err(Refused::NotNow(s)),
    }
}

/// "Yes", bound to `agent` (the object checked it is the person's own):
/// a new attempt at its stage; or, archived (phase 2), the import of the
/// stage kept for it.
pub fn yes(m: &Move, agent: &str, now: i64) -> Outcome {
    match m.state {
        MoveState::Open | MoveState::Later | MoveState::Never => {
            let mut to = m.attempt_again(MoveState::Asked, Phase::P1, now);
            to.agent = Some(agent.into());
            step(to)
        }
        MoveState::Archived => {
            let staged = m.staged_ms.unwrap_or(m.since_ms);
            Ok(Some(m.importing(agent.into(), now, staged)))
        }
        MoveState::Asked | MoveState::Staging | MoveState::Importing if m.agent.as_deref() == Some(agent) => Ok(None),
        s => Err(Refused::NotNow(s)),
    }
}

// ---- the mover's reports

/// A report on its current command.
pub fn report(m: &Move, r: &Report, now: i64) -> Outcome {
    let Some(act) = m.command else { return Err(Refused::Stale) };
    if m.command_id().as_deref() != Some(r.command.as_str()) {
        return Err(Refused::Stale);
    }
    if r.seq <= m.seq {
        return Ok(None);
    }
    let taken = Move { seq: r.seq, ..m.clone() };
    let unfit = Err(Refused::Unfit("this event does not fit the command it reports on"));
    // each event's fields, as the flat report carries them
    let missing = |what: bool| if what { Ok(()) } else { Err(Refused::Unfit("an event lacks a field it needs: progress bytes and files, staged manifest_sha256, bytes and files, failed step and why, retired receipt")) };
    let kind = r.kind.as_str();
    match kind {
        "progress" => missing(r.bytes.is_some() && r.files.is_some())?,
        "staged" => missing(r.manifest_sha256.is_some() && r.bytes.is_some() && r.files.is_some())?,
        "failed" => missing(r.step.is_some() && r.why.is_some())?,
        "retired" => missing(r.receipt.is_some())?,
        "stopped" | "restarted" => {}
        _ => return Err(Refused::Unfit("a report's kind is stopped, progress, staged, failed, retired or restarted")),
    }
    match (act, kind) {
        (Act::StopAndStage, "stopped" | "progress") => match m.state {
            MoveState::Asked => step(Move { progress_ms: Some(now), ..taken.moved(MoveState::Staging, now) }),
            MoveState::Staging | MoveState::Archiving => step(Move { progress_ms: Some(now), ..taken }),
            s => Err(Refused::NotNow(s)),
        },
        (Act::StopAndStage, "staged") => match m.state {
            MoveState::Asked | MoveState::Staging => {
                let agent = m.agent.clone().ok_or(Refused::Unfit("a phase 1 stage is of a move bound to its agent"))?;
                Ok(Some(taken.importing(agent, now, now)))
            }
            // phase 2: the old agent retired next, as archived
            MoveState::Archiving => step(Move { command: Some(Act::Retire(Mark::Archived)), seq: 0, progress_ms: Some(now), staged_ms: Some(now), ..taken }),
            s => Err(Refused::NotNow(s)),
        },
        (Act::StopAndStage, "failed") => match m.state {
            MoveState::Asked | MoveState::Staging => Ok(Some(taken.stage_again_or_hold(HeldReason::StageFailed, now))),
            MoveState::Archiving => Ok(Some(taken.stage_again_or_hold(HeldReason::ArchiveFailed, now))),
            s => Err(Refused::NotNow(s)),
        },
        (Act::Retire(_), "retired") => match m.state {
            MoveState::Retiring => step(Move { command: None, seq: 0, ..taken.moved(MoveState::Moved, now) }),
            MoveState::Archiving => step(Move { command: None, seq: 0, ..taken.moved(MoveState::Archived, now) }),
            s => Err(Refused::NotNow(s)),
        },
        // a retirement that failed is asked again: its command stays, the
        // mover tries it again, and its deadline alone holds it for staff.
        // The moved mark it ends in is what finite.computer redirects by.
        (Act::Retire(_), "failed") => match m.state {
            MoveState::Retiring | MoveState::Archiving => step(taken),
            s => Err(Refused::NotNow(s)),
        },
        (Act::Restart, "restarted" | "failed") => step(Move { command: None, seq: 0, ..taken }),
        _ => unfit,
    }
}

// ---- the mover's hand-off of an old agent's secrets (F11)

/// A hand-off's kinds, within its bounds: its command's id at most
/// `TEXT_MAX_BYTES`, 1 to 3 items, each kind once, each value 1 to
/// `HANDOFF_VALUE_MAX_BYTES`. Says nothing of a value.
pub fn handoff_kinds(h: &Handoff) -> Result<Vec<SecretKind>, Refused> {
    let command_ok = !h.command.is_empty() && h.command.len() <= TEXT_MAX_BYTES && h.command.bytes().all(|b| b.is_ascii_graphic());
    if !command_ok || h.items.is_empty() || h.items.len() > SecretKind::ALL.len() {
        return Err(Refused::Unfit("a hand-off names its command and 1 to 3 secrets"));
    }
    if h.items.iter().any(|i| i.value.is_empty() || i.value.len() > HANDOFF_VALUE_MAX_BYTES) {
        return Err(Refused::Unfit("a secret's value is 1 to 4096 bytes"));
    }
    let kinds: Vec<SecretKind> = h.items.iter().map(|i| i.kind).collect();
    if kinds.iter().enumerate().any(|(n, k)| kinds[..n].contains(k)) {
        return Err(Refused::Unfit("a hand-off names each kind once"));
    }
    Ok(kinds)
}

/// Whether the mover may hand off `kinds` of `m`'s old agent for
/// `command`: its current command (409 otherwise), whose `secrets` name
/// each (400), while its old agent is being stopped or staged (409).
pub fn handoff(m: &Move, command: &str, has: &Has, kinds: &[SecretKind]) -> Result<(), Refused> {
    if m.command_id().as_deref() != Some(command) {
        return Err(Refused::Stale);
    }
    let asked = m.secrets(has);
    if !kinds.iter().all(|k| asked.contains(k)) {
        return Err(Refused::Unfit("a hand-off names only the secrets its command asks for"));
    }
    match m.state {
        MoveState::Asked | MoveState::Staging => Ok(()),
        s => Err(Refused::NotNow(s)),
    }
}

/// A secret handed off, as its person's Vault keeps it: its name there,
/// bound to the new agent's fragment, toward the hosts its kind goes to.
/// `None`: a Nostr key that is not one (the Vault takes no other value).
///
/// - Telegram's token, toward `api.telegram.org`: moved from there into
///   the new agent's Telegram connector by Settings (`fromVault`).
/// - OpenRouter's key, toward `openrouter.ai`.
/// - The Nostr key, as 64 hex, to sign with (NIP-98) and with no host
///   yet: kept, never sent, until something its person sets up asks for
///   one.
///
/// Its name is its kind's and the agent's, at most 32 bytes: `telegram-
/// <label>--<suffix>`, the label cut short to fit.
pub fn vault_secret(kind: SecretKind, fragment: &str, value: &str) -> Option<(String, PutSecret)> {
    let (word, hosts, place, value) = match kind {
        SecretKind::TelegramBotToken => ("telegram", vec!["api.telegram.org".to_string()], Place::default(), value.to_string()),
        SecretKind::OpenrouterKey => ("openrouter", vec!["openrouter.ai".to_string()], Place::default(), value.to_string()),
        SecretKind::NostrNsec => ("nostr", vec![], Place { sign: Some("nip98".into()), ..Place::default() }, crate::npub::secret_hex(value)?),
    };
    let (label, suffix) = fragment_proto::split_fragment_name(fragment)?;
    let room = 32 - word.len() - 3 - suffix.len();
    let label = label[..label.len().min(room)].trim_end_matches('-');
    let name = format!("{word}-{label}--{suffix}");
    Some((name, PutSecret { value, hosts, place, agents: vec![fragment.to_string()] }))
}

// ---- the mover's upload

/// Whether the mover may upload `stage` now: the stage its old agent's
/// current `stop_and_stage` names (another, 403), while it stages, the
/// old agent stopped (`staging`, or phase 2's `archiving` once stopped:
/// 409 otherwise). It reads nothing either way.
pub fn uploadable(m: &Move, stage: &str) -> Result<(), Refused> {
    if m.command != Some(Act::StopAndStage) || m.stage().as_deref() != Some(stage) {
        return Err(Refused::NotYours);
    }
    match (m.state, m.progress_ms) {
        (MoveState::Staging, _) | (MoveState::Archiving, Some(_)) => Ok(()),
        (s, _) => Err(Refused::NotNow(s)),
    }
}

// ---- the new agent

/// Who may read stage `stage`: the bound agent, its current stage, while
/// it imports (the order of the refusals: who, which, when).
fn its_stage(m: &Move, agent: &str, stage: &str) -> Result<(), Refused> {
    if m.agent.as_deref() != Some(agent) {
        return Err(Refused::NotYours);
    }
    if m.stage().as_deref() != Some(stage) {
        return Err(Refused::NoStage);
    }
    Ok(())
}

/// Whether the agent may read its stage now: its own, while importing.
pub fn readable(m: &Move, agent: &str, stage: &str) -> Result<(), Refused> {
    its_stage(m, agent, stage)?;
    match m.state {
        MoveState::Importing => Ok(()),
        s => Err(Refused::NotNow(s)),
    }
}

/// The agent opened its stage.
pub fn open(m: &Move, agent: &str, stage: &str) -> Outcome {
    readable(m, agent, stage)?;
    match m.opened {
        true => Ok(None),
        false => step(Move { opened: true, ..m.clone() }),
    }
}

/// The agent says its import ended: well, the old agent is retired as
/// migrated (a phase 2 one is marked again); not, staff take it. Its reads
/// end either way; a phase 1 stage is swept, a phase 2 one kept to its
/// expiry (`swept`). The same answer again changes nothing.
pub fn done(m: &Move, agent: &str, stage: &str, d: &StageDone, now: i64) -> Outcome {
    its_stage(m, agent, stage)?;
    match (m.state, d.ok) {
        (MoveState::Importing, ok) => {
            let mut effects = vec![];
            let from = m.clone().swept(&mut effects);
            let to = match ok {
                true => Move { command: Some(Act::Retire(Mark::Migrated)), seq: 0, ..from.moved(MoveState::Retiring, now) },
                false => from.held(HeldReason::ImportFailed, now),
            };
            Ok(Some(Step { to, effects }))
        }
        (MoveState::Retiring | MoveState::Moved, true) => Ok(None),
        (MoveState::Held, false) if m.reason == Some(HeldReason::ImportFailed) => Ok(None),
        (s, _) => Err(Refused::NotNow(s)),
    }
}

// ---- the supervisor

/// Its deadline passed (`deadline` at or before `now`): what happens then.
/// `None` when nothing is due.
pub fn tick(m: &Move, now: i64) -> Option<Step> {
    let due = m.deadline().is_some_and(|d| d <= now);
    if !due {
        return None;
    }
    // a stage past its keep goes first, whatever the state
    if m.stage_expires().is_some_and(|at| at <= now) {
        let mut effects = vec![];
        let to = m.clone().expired(&mut effects);
        // what else is due is looked at again at the next pass
        return Some(Step { to, effects });
    }
    let to = match m.state {
        MoveState::Later => m.moved(MoveState::Open, now),
        MoveState::Asked => m.held(HeldReason::MoverDown, now),
        MoveState::Staging => return Some(m.stage_again_or_hold(HeldReason::StageFailed, now)),
        MoveState::Importing if !m.opened && m.routines < ROUTINES_MAX => {
            let to = Move { routine_ms: Some(now), routines: m.routines + 1, ..m.clone() };
            let (agent, stage) = (to.agent.clone().expect("an import has its agent"), to.stage().expect("an import is of an attempt"));
            return Some(Step { to, effects: vec![Effect::Routine { agent, stage }] });
        }
        MoveState::Importing if !m.opened => m.held(HeldReason::NotOpened, now),
        MoveState::Importing => m.held(HeldReason::ImportTimedOut, now),
        MoveState::Retiring => m.held(HeldReason::RetireTimedOut, now),
        MoveState::Archiving => match &m.command {
            Some(Act::Retire(_)) => m.held(HeldReason::ArchiveFailed, now),
            _ => return Some(m.stage_again_or_hold(HeldReason::ArchiveFailed, now)),
        },
        MoveState::Archived => {
            let mut effects = vec![];
            let to = m.clone().expired(&mut effects).moved(MoveState::Expired, now);
            return Some(Step { to, effects });
        }
        _ => return None,
    };
    Some(Step { to, effects: vec![] })
}

/// When the object's alarm next has work: the earliest deadline of any
/// move (`deadline`), or the cutoff while an unasked move waits for it and
/// there is room to archive one. Past the cutoff with every slot taken,
/// an archiving move's own deadline wakes it (or its report, which sets
/// the alarm again): not the past cutoff, at once, over and over.
pub fn next_wake(deadline: Option<i64>, cutoff_ms: Option<i64>, unasked: u64, archiving: usize, now: i64) -> Option<i64> {
    let cutoff = cutoff_ms.filter(|c| unasked > 0 && (*c > now || archiving < ARCHIVING_MAX));
    [deadline, cutoff].into_iter().flatten().min()
}

/// Phase 2, at the cutoff (`cutoff_ms`): a move whose person never said
/// Yes is staged and retired, unasked, its stage kept `ARCHIVE_KEEP_MS`
/// past the cutoff. The object takes at most `ARCHIVING_MAX` at once.
pub fn cutoff(m: &Move, cutoff_ms: i64, now: i64) -> Option<Step> {
    (m.unasked() && now >= cutoff_ms).then(|| {
        let mut to = m.attempt_again(MoveState::Archiving, Phase::P2, now);
        to.agent = None;
        to.expires_ms = Some(cutoff_ms + ARCHIVE_KEEP_MS);
        Step { to, effects: vec![] }
    })
}

// ---- operators

/// Asked again, from where it was held: a new attempt at its stage, or its
/// retirement again (an archive's, its stage kept), or its import's
/// routine again (phase 2's kept stage).
pub fn retry(m: &Move, now: i64) -> Outcome {
    if m.state != MoveState::Held {
        return Err(Refused::NotNow(m.state));
    }
    let mut effects = vec![];
    let to = match (m.held_from, m.phase) {
        (Some(MoveState::Retiring), _) => Move { command: Some(Act::Retire(Mark::Migrated)), seq: 0, ..m.moved(MoveState::Retiring, now) },
        // an archive staged whose retirement's word never came: its
        // retirement again, its stage kept (the mover may have retired the
        // old agent and lost its receipt: the stage is the only copy then)
        (Some(MoveState::Archiving), _) if m.staged_ms.is_some() => Move { command: Some(Act::Retire(Mark::Archived)), seq: 0, progress_ms: Some(now), ..m.moved(MoveState::Archiving, now) },
        (Some(MoveState::Archiving), _) => m.clone().swept(&mut effects).attempt_again(MoveState::Archiving, Phase::P2, now),
        (Some(MoveState::Importing), Phase::P2) => {
            let agent = m.agent.clone().ok_or(Refused::Unfit("a held import has its agent"))?;
            let staged = m.staged_ms.ok_or(Refused::Unfit("its phase 2 stage is gone, past its expiry: nothing is left to import"))?;
            return Ok(Some(m.importing(agent, now, staged)));
        }
        _ => {
            if m.agent.is_none() {
                return Err(Refused::Unfit("a move is staged again only for the agent it was bound to"));
            }
            m.clone().swept(&mut effects).attempt_again(MoveState::Asked, Phase::P1, now)
        }
    };
    Ok(Some(Step { to, effects }))
}

/// The old agent started again and offered again: a person who changed
/// their mind after Yes, or a move staff give up on. Never once its
/// retirement began.
pub fn restart_old(m: &Move, now: i64) -> Outcome {
    let restartable = match m.state {
        MoveState::Asked | MoveState::Staging | MoveState::Importing => m.phase == Phase::P1,
        MoveState::Held => m.phase == Phase::P1 && matches!(m.held_from, Some(MoveState::Asked | MoveState::Staging | MoveState::Importing)),
        _ => false,
    };
    if !restartable {
        return Err(Refused::NotNow(m.state));
    }
    let mut effects = vec![];
    let from = m.clone().swept(&mut effects);
    let to = Move { agent: None, command: Some(Act::Restart), seq: 0, progress_ms: None, routine_ms: None, routines: 0, opened: false, ..from.moved(MoveState::Open, now) };
    Ok(Some(Step { to, effects }))
}

/// Retired as migrated: a held move staff finished by hand.
pub fn retire(m: &Move, now: i64) -> Outcome {
    match m.state {
        MoveState::Held => {
            let mut effects = vec![];
            let to = Move { command: Some(Act::Retire(Mark::Migrated)), seq: 0, ..m.clone().swept(&mut effects).moved(MoveState::Retiring, now) };
            Ok(Some(Step { to, effects }))
        }
        s => Err(Refused::NotNow(s)),
    }
}

/// Out of the move by staff, saying why: offered no more, never archived,
/// whatever a roster says (the will-not-move list). A held move has no
/// command left; a restart pending stays, so its old agent still starts.
/// The same why again changes nothing; another replaces it.
pub fn withdraw(m: &Move, why: &str, now: i64) -> Outcome {
    let why = why.trim();
    if why.is_empty() || why.len() > WHY_MAX_BYTES {
        return Err(Refused::Why);
    }
    match m.state {
        MoveState::Withdrawn if m.by_staff && m.why.as_deref() == Some(why) => Ok(None),
        MoveState::Withdrawn if m.by_staff => step(Move { why: Some(why.into()), ..m.clone() }),
        MoveState::Listed | MoveState::Open | MoveState::Later | MoveState::Never | MoveState::Held | MoveState::Withdrawn => {
            let mut effects = vec![];
            let to = Move { by_staff: true, why: Some(why.into()), ..m.clone().swept(&mut effects).moved(MoveState::Withdrawn, now) };
            Ok(Some(Step { to, effects }))
        }
        s => Err(Refused::NotNow(s)),
    }
}

/// Staff's withdrawal taken back: offered again, `open` for a person here
/// (`bound`), else `listed`. A row withdrawn ahead of any roster is
/// forgotten; one a roster dropped stays withdrawn, the roster's, until a
/// roster lists it. Anything else is not staff's to take back: nothing.
/// One whose move began (a Yes binds its agent; the cutoff makes it phase
/// 2) is refused: its old agent may be stopped or retired, and a phase 2
/// stage is the only copy left, which an offer's Yes would stage afresh.
pub fn include(m: &Move, bound: bool, now: i64) -> Outcome {
    if m.state != MoveState::Withdrawn || !m.by_staff {
        return Ok(None);
    }
    if m.agent.is_some() || m.phase == Phase::P2 {
        return Err(Refused::Begun);
    }
    if !m.rostered() {
        return Ok(Some(Step { to: m.clone(), effects: vec![Effect::Forget] }));
    }
    let to = match m.dropped {
        true => m.clone(),
        false => m.moved(if bound { MoveState::Open } else { MoveState::Listed }, now),
    };
    step(Move { by_staff: false, why: None, ..to })
}

// ---- what its person is told (F4)

/// An open move, as its person is asked (`GET /api/notices`): a notice that
/// must be answered, in the server's words. Yes opens `page` (the
/// platform's `/move`, which asks what the import needs and says Yes);
/// Not now and Never post to the person's own routes. `picture`: the
/// roster has one, which the platform reads for them.
pub fn offer(old: &str, name: &str, picture: bool, page: &str) -> Notice {
    let post = |label: &str, verb: &str| NoticeAction { label: label.into(), post: Some(format!("/api/migrations/offers/{old}/{verb}")), body: None, open: None };
    let name = match name.trim() {
        "" => "your agent",
        n => n,
    };
    Notice {
        id: format!("move:{old}"),
        kind: "move_offer".into(),
        level: NoticeLevel::Info,
        title: format!("Bring {name} over from finite.computer?"),
        text: Some("You'll get a month free.".into()),
        detail: None,
        until: None,
        actions: vec![NoticeAction::open("Yes", page), post("Not now", "later"), post("Never", "never")],
        must_answer: true,
        image: picture.then(|| format!("/api/migrations/offers/{old}/picture")),
    }
}

/// Where `m` is, as `/move` tells its person (`Offer.said`), `name` its
/// old agent's: plain, and only what its state says.
pub fn said(m: &Move, name: &str) -> String {
    let name = match name.trim() {
        "" => "your agent",
        n => n,
    };
    match m.state {
        MoveState::Listed | MoveState::Open | MoveState::Later => format!("{name} is on finite.computer."),
        MoveState::Never => "You said never.".into(),
        MoveState::Asked => format!("Stopping {name} on finite.computer…"),
        MoveState::Staging | MoveState::Archiving => format!("Copying {name}…"),
        MoveState::Importing => format!("Bringing {name} over…"),
        MoveState::Retiring | MoveState::Moved => "Done.".into(),
        MoveState::Held => "Paused: someone from Finite will look at it and be in touch.".into(),
        MoveState::Archived => format!("{name} is archived. You can still bring it over."),
        MoveState::Expired => "Its archive was deleted.".into(),
        MoveState::Withdrawn => "No longer offered.".into(),
    }
}

#[cfg(test)]
mod tests {
    //! Each transition: valid (it moves), invalid (refused, nothing
    //! changes), replay (the same input again changes nothing) and restart
    //! (a move stored and read back, as a restarted object reads it, goes
    //! on the same).
    use super::*;


    const T0: i64 = 1_800_000_000_000;
    const AGENT: &str = "npub1agent";
    const WHY: &str = "a staff test account";

    /// Stored and read back: a restarted object's move, from its columns
    /// as SQLite answers them.
    fn restarted(m: &Move) -> Move {
        let row: serde_json::Map<String, Value> = COLUMNS
            .iter()
            .zip(m.values())
            .map(|(c, v)| {
                let v = match v {
                    Col::Null => Value::Null,
                    Col::Int(n) => Value::from(n),
                    Col::Text(t) => Value::from(t),
                };
                (c.to_string(), v)
            })
            .collect();
        let back = Move::read(&Value::Object(row)).expect("a move reads back");
        assert_eq!(&back, m, "a move reads back as it was");
        back
    }

    fn to(o: Outcome) -> Move {
        o.expect("taken").expect("a change").to
    }

    fn open_move() -> Move {
        Move::listed("prj_1", "user_1", true, T0)
    }

    /// A report on `m`'s current command, with the fields its event needs.
    fn report_on(m: &Move, seq: u64, event: &str) -> Report {
        let mut r = Report::of(&m.command_id().expect("a command"), seq, event);
        match event {
            "progress" => (r.bytes, r.files) = (Some(5), Some(1)),
            "staged" => (r.manifest_sha256, r.bytes, r.files) = (Some("ab".repeat(32)), Some(10), Some(2)),
            "failed" => (r.step, r.why) = (Some("stream".into()), Some("x".into())),
            "retired" => r.receipt = Some("snap".into()),
            _ => {}
        }
        r
    }

    /// Asked, then stopped: staging.
    fn staging() -> Move {
        let asked = to(yes(&open_move(), AGENT, T0));
        to(report(&asked, &report_on(&asked, 1, "stopped"), T0 + 1))
    }

    fn staged_event() -> &'static str {
        "staged"
    }

    fn importing() -> Move {
        let s = staging();
        to(report(&s, &report_on(&s, 2, staged_event()), T0 + 10))
    }

    fn ok() -> StageDone {
        StageDone { ok: true, report_sha256: None, why: None }
    }

    #[test]
    fn a_roster_row_is_listed_or_open_and_binds_once() {
        let listed = Move::listed("prj_1", "user_1", false, T0);
        assert_eq!(listed.state, MoveState::Listed);
        assert_eq!(listed.deadline(), None);
        let bound = bind(&listed, T0 + 1).expect("binds");
        assert_eq!(bound.to.state, MoveState::Open);
        // replay: bound already
        assert_eq!(bind(&bound.to, T0 + 2), None);
        assert_eq!(bind(&restarted(&bound.to), T0 + 2), None);
        // invalid: only a listed row binds
        assert_eq!(bind(&staging(), T0 + 2), None);
    }

    #[test]
    fn not_now_comes_back_a_day_later() {
        let m = open_move();
        let later_m = to(later(&m, T0 + 5));
        assert_eq!((later_m.state, later_m.deadline()), (MoveState::Later, Some(T0 + 5 + LATER_MS)));
        assert_eq!(later(&later_m, T0 + 6), Ok(None), "replay");
        assert_eq!(tick(&later_m, T0 + 5 + LATER_MS - 1), None, "not yet");
        let back = tick(&restarted(&later_m), T0 + 5 + LATER_MS).expect("due");
        assert_eq!(back.to.state, MoveState::Open);
        assert_eq!(later(&staging(), T0), Err(Refused::NotNow(MoveState::Staging)), "invalid");
        // a long sleep: one pass catches it up
        assert_eq!(tick(&later_m, T0 + 30 * DAY_MS).expect("due").to.state, MoveState::Open);
    }

    #[test]
    fn never_until_asked_again() {
        let m = open_move();
        let n = to(never(&m, T0));
        assert_eq!((n.state, n.deadline()), (MoveState::Never, None));
        assert_eq!(never(&n, T0), Ok(None), "replay");
        assert_eq!(never(&to(later(&m, T0)), T0).map(|s| s.map(|s| s.to.state)), Ok(Some(MoveState::Never)), "from later too");
        let again = to(ask_again(&restarted(&n), T0 + 1));
        assert_eq!(again.state, MoveState::Open);
        assert_eq!(ask_again(&again, T0 + 2), Ok(None), "replay");
        assert_eq!(ask_again(&Move::listed("o", "u", false, T0), T0), Err(Refused::NotNow(MoveState::Listed)), "invalid");
        assert_eq!(never(&importing(), T0), Err(Refused::NotNow(MoveState::Importing)), "invalid");
    }

    #[test]
    fn yes_asks_the_mover_to_stop_and_stage() {
        let a = to(yes(&open_move(), AGENT, T0));
        assert_eq!((a.state, a.attempt, a.phase, a.agent.as_deref()), (MoveState::Asked, 1, Phase::P1, Some(AGENT)));
        assert_eq!(a.command, Some(Act::StopAndStage));
        let c = a.command().expect("its command, as the mailbox answers it");
        assert_eq!((c.kind, c.phase, c.stage.as_deref(), c.mark), (CommandKind::StopAndStage, Some(Phase::P1), Some("prj_1.p1.1"), None));
        assert_eq!(a.command_id().as_deref(), Some("prj_1:1:stop_and_stage"));
        assert_eq!(a.deadline(), Some(T0 + STOP_MS));
        assert_eq!(yes(&a, AGENT, T0 + 1), Ok(None), "replay by the same agent");
        assert_eq!(yes(&restarted(&a), "npub1other", T0 + 1), Err(Refused::NotNow(MoveState::Asked)), "another agent");
        assert_eq!(yes(&Move::listed("o", "u", false, T0), AGENT, T0), Err(Refused::NotNow(MoveState::Listed)), "unbound");
        assert_eq!(to(yes(&to(never(&open_move(), T0)), AGENT, T0)).state, MoveState::Asked, "from never too");
    }

    #[test]
    fn the_mover_down_holds_the_move() {
        let a = to(yes(&open_move(), AGENT, T0));
        assert_eq!(tick(&a, T0 + STOP_MS - 1), None);
        let h = tick(&restarted(&a), T0 + STOP_MS).expect("due").to;
        assert_eq!((h.state, h.reason, h.held_from, h.command), (MoveState::Held, Some(HeldReason::MoverDown), Some(MoveState::Asked), None));
        assert_eq!(h.deadline(), None, "held waits for staff");
    }

    #[test]
    fn stopped_starts_staging_and_progress_keeps_it_alive() {
        let a = to(yes(&open_move(), AGENT, T0));
        let s = to(report(&a, &report_on(&a, 1, "stopped"), T0 + 1));
        assert_eq!((s.state, s.seq, s.progress_ms), (MoveState::Staging, 1, Some(T0 + 1)));
        assert_eq!(s.deadline(), Some(T0 + 1 + STALL_MS));
        // replay: the same seq, or an older one, changes nothing
        assert_eq!(report(&s, &report_on(&s, 1, "stopped"), T0 + 2), Ok(None));
        let p = to(report(&restarted(&s), &report_on(&s, 2, "progress"), T0 + 20 * MINUTE_MS));
        assert_eq!(p.deadline(), Some(T0 + 20 * MINUTE_MS + STALL_MS));
        // progress from asked: stopped first, implied
        assert_eq!(to(report(&a, &report_on(&a, 1, "progress"), T0 + 1)).state, MoveState::Staging);
        // invalid: another command's id is stale (409), an event that does
        // not fit its command is 400; neither changes it
        let stale = Report::of("prj_1:0:stop_and_stage", 9, "stopped");
        assert_eq!(report(&s, &stale, T0), Err(Refused::Stale));
        assert!(matches!(report(&s, &report_on(&s, 9, "retired"), T0), Err(Refused::Unfit(_))));
        assert_eq!(report(&open_move(), &stale, T0), Err(Refused::Stale), "no command at all");
    }

    #[test]
    fn a_stalled_stage_is_tried_again_then_held() {
        let s = staging();
        // stalled: attempt 2, the first's prefix swept
        let again = tick(&s, s.deadline().unwrap()).expect("due");
        assert_eq!((again.to.state, again.to.attempt), (MoveState::Asked, 2));
        assert_eq!(again.effects, vec![Effect::Sweep { prefix: "p1/prj_1/1/".into() }]);
        assert_eq!(again.to.command_id().as_deref(), Some("prj_1:2:stop_and_stage"));
        // the stale attempt's report is refused now
        assert_eq!(report(&again.to, &report_on(&s, 5, staged_event()), T0), Err(Refused::Stale));
        // a whole staging is bounded too, however it progresses
        let mut long = staging();
        long.progress_ms = Some(long.since_ms + STAGE_MS - 1);
        assert_eq!(long.deadline(), Some(long.since_ms + STAGE_MS));
        // failed: the same as a stall; the third time, held
        let mut m = staging();
        for attempt in 1..=ATTEMPTS_MAX {
            assert_eq!(m.attempt, attempt);
            let st = to(report(&m, &report_on(&m, 9, "failed"), T0));
            if attempt < ATTEMPTS_MAX {
                assert_eq!(st.state, MoveState::Asked);
                m = to(report(&st, &report_on(&st, 1, "stopped"), T0));
            } else {
                assert_eq!((st.state, st.reason, st.held_from), (MoveState::Held, Some(HeldReason::StageFailed), Some(MoveState::Staging)));
            }
        }
    }

    #[test]
    fn staged_starts_the_import_with_its_routine() {
        let s = staging();
        let st = report(&s, &report_on(&s, 2, staged_event()), T0 + 10).unwrap().unwrap();
        let i = &st.to;
        assert_eq!((i.state, i.command, i.staged_ms, i.routines), (MoveState::Importing, None, Some(T0 + 10), 1));
        assert_eq!(st.effects, vec![Effect::Routine { agent: AGENT.into(), stage: "prj_1.p1.1".into() }]);
        assert_eq!(i.deadline(), Some(T0 + 10 + OPEN_MS));
        // replay: the command is gone, so a report on it is stale
        assert_eq!(report(i, &report_on(&s, 3, staged_event()), T0), Err(Refused::Stale));
    }

    #[test]
    fn an_unopened_stage_gets_its_routine_once_more_then_held() {
        let i = importing();
        let again = tick(&restarted(&i), T0 + 10 + OPEN_MS).expect("due");
        assert_eq!(again.effects, vec![Effect::Routine { agent: AGENT.into(), stage: "prj_1.p1.1".into() }]);
        assert_eq!((again.to.routines, again.to.state), (2, MoveState::Importing));
        let at = again.to.deadline().unwrap();
        let held = tick(&again.to, at).expect("due").to;
        assert_eq!((held.state, held.reason), (MoveState::Held, Some(HeldReason::NotOpened)));
        assert_eq!(held.staged_ms, Some(T0 + 10), "its stage kept for staff");
        assert_eq!(held.deadline(), Some(T0 + 10 + KEEP_MS), "until its keep");
    }

    #[test]
    fn only_the_bound_agent_reads_its_current_stage_while_importing() {
        let i = importing();
        assert_eq!(readable(&i, AGENT, "prj_1.p1.1"), Ok(()));
        assert_eq!(readable(&i, "npub1other", "prj_1.p1.1"), Err(Refused::NotYours));
        assert_eq!(readable(&i, AGENT, "prj_1.p1.2"), Err(Refused::NoStage));
        assert_eq!(readable(&staging(), AGENT, "prj_1.p1.1"), Err(Refused::NotNow(MoveState::Staging)));
        let o = to(open(&i, AGENT, "prj_1.p1.1"));
        assert!(o.opened);
        assert_eq!(open(&o, AGENT, "prj_1.p1.1"), Ok(None), "replay");
        assert_eq!(o.deadline(), Some(T0 + 10 + DONE_MS), "opened: done within a day of its stage");
        let held = tick(&restarted(&o), T0 + 10 + DONE_MS).expect("due").to;
        assert_eq!(held.reason, Some(HeldReason::ImportTimedOut));
    }

    #[test]
    fn done_retires_the_old_agent_and_sweeps_the_stage() {
        let i = to(open(&importing(), AGENT, "prj_1.p1.1"));
        let d = done(&i, AGENT, "prj_1.p1.1", &ok(), T0 + 100).unwrap().unwrap();
        assert_eq!(d.effects, vec![Effect::Sweep { prefix: "p1/prj_1/1/".into() }]);
        assert_eq!((d.to.state, d.to.command, d.to.staged_ms), (MoveState::Retiring, Some(Act::Retire(Mark::Migrated)), None));
        assert_eq!(d.to.command_id().as_deref(), Some("prj_1:1:retire_migrated"));
        assert_eq!(readable(&d.to, AGENT, "prj_1.p1.1"), Err(Refused::NotNow(MoveState::Retiring)), "its routes refuse at once");
        assert_eq!(done(&restarted(&d.to), AGENT, "prj_1.p1.1", &ok(), T0 + 101), Ok(None), "replay");
        let bad = StageDone { ok: false, report_sha256: None, why: Some("x".into()) };
        assert_eq!(done(&d.to, AGENT, "prj_1.p1.1", &bad, T0), Err(Refused::NotNow(MoveState::Retiring)), "the other answer after");
        assert_eq!(done(&i, "npub1other", "prj_1.p1.1", &ok(), T0), Err(Refused::NotYours));
        // retired: moved
        let r = d.to;
        let m = to(report(&r, &report_on(&r, 1, "retired"), T0 + 200));
        assert_eq!((m.state, m.command, m.deadline()), (MoveState::Moved, None, None));
        assert_eq!(report(&m, &report_on(&r, 2, "retired"), T0), Err(Refused::Stale), "done with");
    }

    #[test]
    fn a_failed_import_is_held_and_swept() {
        let i = importing();
        let bad = StageDone { ok: false, report_sha256: None, why: Some("x".into()) };
        let d = done(&i, AGENT, "prj_1.p1.1", &bad, T0).unwrap().unwrap();
        assert_eq!((d.to.state, d.to.reason, d.to.staged_ms), (MoveState::Held, Some(HeldReason::ImportFailed), None));
        assert_eq!(d.effects.len(), 1);
        assert_eq!(done(&restarted(&d.to), AGENT, "prj_1.p1.1", &bad, T0), Ok(None), "replay");
        assert_eq!(done(&d.to, AGENT, "prj_1.p1.1", &ok(), T0), Err(Refused::NotNow(MoveState::Held)));
    }

    #[test]
    fn a_failed_retirement_is_asked_again_until_its_deadline() {
        let r = done(&importing(), AGENT, "prj_1.p1.1", &ok(), T0).unwrap().unwrap().to;
        // failed: still retiring, its command still the mover's, its deadline unmoved
        let f = to(report(&r, &report_on(&r, 1, "failed"), T0 + 5));
        assert_eq!((f.state, f.command_id().as_deref(), f.deadline()), (MoveState::Retiring, Some("prj_1:1:retire_migrated"), r.deadline()));
        assert_eq!(report(&f, &report_on(&r, 1, "failed"), T0), Ok(None), "replay");
        // the next try goes through: moved
        let m = to(report(&restarted(&f), &report_on(&r, 2, "retired"), T0 + 6));
        assert_eq!(m.state, MoveState::Moved);
        // never through, its day passes: held, and staff retry it as the same command
        let h = tick(&f, T0 + RETIRE_MS).expect("due").to;
        assert_eq!((h.state, h.reason), (MoveState::Held, Some(HeldReason::RetireTimedOut)));
        let again = to(retry(&h, T0 + 1));
        assert_eq!((again.state, again.command_id().as_deref()), (MoveState::Retiring, Some("prj_1:1:retire_migrated")));
        // phase 2's retirement the same, held at its own deadline
        let a = cutoff(&Move::listed("o", "u", false, T0), T0, T0).unwrap().to;
        let s = to(report(&a, &report_on(&a, 1, staged_event()), T0 + 1));
        let f = to(report(&s, &report_on(&s, 1, "failed"), T0 + 2));
        assert_eq!((f.state, f.command), (MoveState::Archiving, Some(Act::Retire(Mark::Archived))));
        assert_eq!(tick(&f, T0 + 1 + RETIRE_MS).expect("due").to.reason, Some(HeldReason::ArchiveFailed));
    }

    #[test]
    fn a_stage_is_kept_fourteen_days_done_or_not() {
        let held = tick(&importing(), T0 + 10 + OPEN_MS).and_then(|s| tick(&s.to, s.to.deadline().unwrap())).expect("held").to;
        let keep = T0 + 10 + KEEP_MS;
        assert_eq!(tick(&held, keep - 1), None);
        let swept = tick(&restarted(&held), keep).expect("due");
        assert_eq!(swept.effects, vec![Effect::Sweep { prefix: "p1/prj_1/1/".into() }]);
        assert_eq!((swept.to.state, swept.to.staged_ms, swept.to.deadline()), (MoveState::Held, None, None));
    }

    #[test]
    fn staff_retry_restart_retire_and_withdraw() {
        let held = tick(&to(yes(&open_move(), AGENT, T0)), T0 + STOP_MS).unwrap().to;
        // retry: a new attempt
        let r = to(retry(&held, T0 + 1));
        assert_eq!((r.state, r.attempt), (MoveState::Asked, 2));
        assert_eq!(retry(&r, T0), Err(Refused::NotNow(MoveState::Asked)), "only a held move");
        // restart-old: offered again, the old agent restarted, unbound
        let back = restart_old(&r, T0 + 2).unwrap().unwrap();
        assert_eq!((back.to.state, back.to.agent.clone(), back.to.command), (MoveState::Open, None, Some(Act::Restart)));
        assert_eq!(back.to.command_id().as_deref(), Some("prj_1:2:restart"));
        let restarted_ = to(report(&back.to, &report_on(&back.to, 1, "restarted"), T0 + 3));
        assert_eq!((restarted_.state, restarted_.command), (MoveState::Open, None));
        // a Yes after: attempt 3, never mixed with the others
        assert_eq!(to(yes(&restarted_, AGENT, T0 + 4)).attempt, 3);
        assert_eq!(restart_old(&open_move(), T0), Err(Refused::NotNow(MoveState::Open)));
        let retiring = done(&importing(), AGENT, "prj_1.p1.1", &ok(), T0).unwrap().unwrap().to;
        assert_eq!(restart_old(&retiring, T0), Err(Refused::NotNow(MoveState::Retiring)), "never once retirement began");
        // retire: a held move finished by hand
        let ret = to(retire(&restarted(&held), T0 + 5));
        assert_eq!((ret.state, ret.command), (MoveState::Retiring, Some(Act::Retire(Mark::Migrated))));
        assert_eq!(retire(&open_move(), T0), Err(Refused::NotNow(MoveState::Open)));
        // withdraw: out, and a roster leaves it out
        let w = to(withdraw(&held, WHY, T0 + 6));
        assert_eq!((w.state, w.by_staff, w.command), (MoveState::Withdrawn, true, None));
        assert_eq!(withdraw(&w, WHY, T0), Ok(None), "replay");
        assert_eq!(relisted(&restarted(&w), "user_1", true, T0), None);
        assert_eq!(withdraw(&staging(), WHY, T0), Err(Refused::NotNow(MoveState::Staging)), "in flight: restart it first");
    }

    #[test]
    fn a_roster_withdraws_the_unasked_and_lists_them_again() {
        let m = open_move();
        let w = withdrawn(&m, T0).expect("withdrawn").to;
        assert_eq!((w.state, w.by_staff), (MoveState::Withdrawn, false));
        assert_eq!(withdrawn(&w, T0), None, "replay");
        assert_eq!(withdrawn(&staging(), T0), None, "in flight: untouched");
        assert_eq!(relisted(&restarted(&w), "user_1", true, T0).expect("again").to.state, MoveState::Open);
        assert_eq!(relisted(&w, "user_1", false, T0).expect("again").to.state, MoveState::Listed);
        assert_eq!(relisted(&m, "user_1", true, T0), None, "only a withdrawn one");
        // a restart pending stays through it, so the old agent still restarts
        let mut pending = open_move();
        pending.command = Some(Act::Restart);
        assert_eq!(withdrawn(&pending, T0).unwrap().to.command, Some(Act::Restart));
    }

    /// The will-not-move list: staff withdraw an old agent no roster has
    /// listed yet; a roster that lists it then fills in its person and
    /// leaves it withdrawn, offered to no one, the cutoff passing it by.
    #[test]
    fn staff_withdraw_ahead_of_the_roster_and_it_stays_out() {
        let w = to(withdraw(&Move::unlisted("prj_9", T0), WHY, T0));
        assert_eq!((w.state, w.by_staff, w.why.as_deref(), w.dropped, w.rostered()), (MoveState::Withdrawn, true, Some(WHY), true, false));
        assert_eq!(w.command(), None, "the mover is told nothing");
        let w = restarted(&w);
        // a roster lists it: its person filled in, still withdrawn
        let listed = relisted(&w, "user_9", true, T0 + 1).expect("its person filled in").to;
        assert_eq!((listed.state, listed.by_staff, listed.workos_id.as_str(), listed.dropped, listed.why.as_deref()), (MoveState::Withdrawn, true, "user_9", false, Some(WHY)));
        assert_eq!(relisted(&listed, "user_9", true, T0 + 2), None, "replay");
        assert_eq!(bind(&listed, T0 + 2), None, "never offered");
        // the person's answers and the cutoff pass it by
        for answer in [later(&listed, T0), never(&listed, T0), ask_again(&listed, T0), yes(&listed, AGENT, T0)] {
            assert_eq!(answer, Err(Refused::NotNow(MoveState::Withdrawn)));
        }
        assert_eq!(cutoff(&listed, T0, T0 + 3), None, "never archived");
        // a roster that drops it, and one that lists it again: withdrawn throughout
        let dropped = withdrawn(&restarted(&listed), T0 + 4).expect("dropped").to;
        assert_eq!((dropped.state, dropped.by_staff, dropped.dropped), (MoveState::Withdrawn, true, true));
        assert_eq!(withdrawn(&dropped, T0 + 5), None, "replay");
        let back = relisted(&dropped, "user_9", false, T0 + 6).expect("listed again").to;
        assert_eq!((back.state, back.by_staff, back.dropped, back.workos_id.as_str()), (MoveState::Withdrawn, true, false, "user_9"));
        // a roster never changes whose it is once it named them
        assert_eq!(relisted(&dropped, "user_other", false, T0 + 6).expect("listed again").to.workos_id, "user_9");
    }

    /// `include` takes back staff's withdrawal alone: offered again (open
    /// for a person here, else listed); a row withdrawn ahead of any roster
    /// forgotten; one a roster dropped left withdrawn, the roster's.
    #[test]
    fn include_takes_back_staffs_withdrawal_alone() {
        // ahead of any roster: forgotten
        let ahead = to(withdraw(&Move::unlisted("prj_9", T0), WHY, T0));
        let gone = include(&restarted(&ahead), false, T0 + 1).expect("taken").expect("a change");
        assert_eq!(gone.effects, vec![Effect::Forget]);
        // listed, open, said Never, its old agent given back: open for a person here, else listed
        let never_m = to(never(&open_move(), T0));
        let restarting = restart_old(&staging(), T0).unwrap().unwrap().to;
        let given_back = to(report(&restarting, &report_on(&restarting, 1, "restarted"), T0));
        for from in [Move::listed("prj_1", "user_1", false, T0), open_move(), never_m, given_back] {
            let w = to(withdraw(&from, WHY, T0 + 1));
            for (bound, state) in [(true, MoveState::Open), (false, MoveState::Listed)] {
                let back = to(include(&restarted(&w), bound, T0 + 2));
                assert_eq!((back.state, back.by_staff, back.why.as_deref(), back.since_ms), (state, false, None, T0 + 2), "from {:?}", from.state);
                assert_eq!(include(&back, bound, T0 + 3), Ok(None), "replay");
            }
        }
        // a roster dropped it, before staff's withdrawal or after: withdrawn still, the roster's
        let roster_first = to(withdraw(&withdrawn(&open_move(), T0).unwrap().to, WHY, T0 + 1));
        let staff_first = withdrawn(&to(withdraw(&open_move(), WHY, T0)), T0 + 1).unwrap().to;
        for w in [roster_first, staff_first] {
            let back = to(include(&restarted(&w), true, T0 + 2));
            assert_eq!((back.state, back.by_staff, back.why.as_deref()), (MoveState::Withdrawn, false, None));
            assert_eq!(include(&back, true, T0 + 3), Ok(None), "the roster's withdrawal is not staff's to take back");
            // and a roster that lists it offers it again
            assert_eq!(relisted(&back, "user_1", true, T0 + 4).expect("again").to.state, MoveState::Open);
        }
        // nothing to take back: in flight, or never withdrawn
        assert_eq!(include(&staging(), true, T0), Ok(None));
        assert_eq!(include(&open_move(), true, T0), Ok(None));
    }

    #[test]
    fn a_withdrawal_says_why_in_bounds() {
        let m = open_move();
        for bad in ["", "   ", &"x".repeat(WHY_MAX_BYTES + 1)] {
            assert_eq!(withdraw(&m, bad, T0), Err(Refused::Why), "{bad:?}");
        }
        let longest = "y".repeat(WHY_MAX_BYTES);
        assert_eq!(to(withdraw(&m, &longest, T0)).why, Some(longest));
        let w = to(withdraw(&m, "  trimmed  ", T0));
        assert_eq!(w.why.as_deref(), Some("trimmed"));
        // the same why again changes nothing; a new one replaces it
        assert_eq!(withdraw(&w, "trimmed", T0 + 1), Ok(None), "replay");
        let again = to(withdraw(&w, "another reason", T0 + 2));
        assert_eq!((again.why.as_deref(), again.since_ms), (Some("another reason"), T0));
        assert_eq!(withdraw(&staging(), "", T0), Err(Refused::Why), "a why is checked first");
    }

    /// A restart pending (staff gave the old agent back) survives staff's
    /// withdrawal: the old agent still starts again.
    #[test]
    fn staffs_withdrawal_keeps_a_restart_pending() {
        let back = restart_old(&staging(), T0 + 2).unwrap().unwrap().to;
        let w = to(withdraw(&back, WHY, T0 + 3));
        assert_eq!((w.state, w.command), (MoveState::Withdrawn, Some(Act::Restart)));
        assert_eq!(w.command_id().as_deref(), Some("prj_1:1:restart"));
    }

    #[test]
    fn phase_two_archives_the_unasked_then_expires_them() {
        let cut = T0 + 1000;
        let m = Move::listed("prj_1", "user_1", false, T0);
        assert_eq!(cutoff(&m, cut, cut - 1), None, "not yet");
        assert_eq!(cutoff(&staging(), cut, cut), None, "asked: untouched");
        let a = cutoff(&m, cut, cut).expect("archiving").to;
        assert_eq!((a.state, a.phase, a.attempt, a.expires_ms), (MoveState::Archiving, Phase::P2, 1, Some(cut + ARCHIVE_KEEP_MS)));
        assert_eq!(a.command().and_then(|c| c.stage), Some("prj_1.p2.1".into()));
        assert_eq!(a.deadline(), Some(cut + STOP_MS));
        // stopped, staged: retired next, as archived
        let s = to(report(&a, &report_on(&a, 1, "stopped"), cut + 1));
        let st = to(report(&restarted(&s), &report_on(&s, 2, staged_event()), cut + 2));
        assert_eq!((st.state, st.command, st.staged_ms), (MoveState::Archiving, Some(Act::Retire(Mark::Archived)), Some(cut + 2)));
        assert_eq!(st.command_id().as_deref(), Some("prj_1:1:retire_archived"));
        let ar = to(report(&st, &report_on(&st, 1, "retired"), cut + 3));
        assert_eq!((ar.state, ar.deadline()), (MoveState::Archived, Some(cut + ARCHIVE_KEEP_MS)), "kept 90 days, not 14");
        // its 90 days: expired, swept
        let e = tick(&restarted(&ar), cut + ARCHIVE_KEEP_MS).expect("due");
        assert_eq!(e.to.state, MoveState::Expired);
        assert_eq!(e.effects, vec![Effect::Sweep { prefix: "p2/prj_1/1/".into() }]);
        assert_eq!(yes(&e.to, AGENT, cut), Err(Refused::NotNow(MoveState::Expired)));
        // a late Yes before then imports the kept stage, then retires it as migrated
        let late = yes(&ar, AGENT, cut + 10).unwrap().unwrap();
        assert_eq!(late.effects, vec![Effect::Routine { agent: AGENT.into(), stage: "prj_1.p2.1".into() }]);
        let d = done(&late.to, AGENT, "prj_1.p2.1", &ok(), cut + 11).unwrap().unwrap();
        assert_eq!((d.to.state, d.to.command), (MoveState::Retiring, Some(Act::Retire(Mark::Migrated))));
        assert_eq!(d.effects, vec![], "kept to its expiry");
    }

    /// Phase 2's two retirements are two commands: as archived at the
    /// cutoff, then as migrated after a late import. A mover that keys its
    /// work by the id does both, the second not answered from the first's
    /// receipt.
    #[test]
    fn a_late_import_retires_under_a_command_of_its_own() {
        let cut = T0;
        let a = cutoff(&Move::listed("o", "u", true, T0), cut, cut).unwrap().to;
        let s = to(report(&a, &report_on(&a, 1, "stopped"), cut + 1));
        let st = to(report(&restarted(&s), &report_on(&s, 2, staged_event()), cut + 2));
        let archived = st.command_id().expect("retired as archived");
        let ar = to(report(&st, &report_on(&st, 1, "retired"), cut + 3));
        let late = to(yes(&ar, AGENT, cut + 10));
        let d = to(done(&late, AGENT, "o.p2.1", &ok(), cut + 11));
        let migrated = d.command_id().expect("retired as migrated");
        assert_ne!(archived, migrated, "two retirements, two commands");
        // the archive's receipt again is stale: it changes nothing
        assert_eq!(report(&d, &Report { command: archived, ..report_on(&d, 1, "retired") }, cut + 12), Err(Refused::Stale));
    }

    /// A phase 2 stage is the only copy left (its old agent is retired), so
    /// it is kept until its expiry whatever its import does: a failed
    /// import keeps it for staff's retry; held past its expiry, it goes,
    /// and a retry is refused.
    #[test]
    fn a_failed_late_import_keeps_its_stage_until_it_expires() {
        let cut = T0;
        let a = cutoff(&Move::listed("o", "u", true, T0), cut, cut).unwrap().to;
        let s = to(report(&a, &report_on(&a, 1, "stopped"), cut + 1));
        let st = to(report(&restarted(&s), &report_on(&s, 2, staged_event()), cut + 2));
        let ar = to(report(&st, &report_on(&st, 1, "retired"), cut + 3));
        let late = to(yes(&ar, AGENT, cut + 10));
        let failed = StageDone { ok: false, report_sha256: None, why: Some("x".into()) };
        let f = done(&late, AGENT, "o.p2.1", &failed, cut + 11).unwrap().unwrap();
        assert_eq!((f.to.state, f.to.reason), (MoveState::Held, Some(HeldReason::ImportFailed)));
        assert_eq!((f.effects.as_slice(), f.to.deadline()), (&[][..], Some(cut + ARCHIVE_KEEP_MS)), "kept, to its expiry");
        let again = to(retry(&f.to, cut + 12));
        assert_eq!((again.state, again.stage().as_deref()), (MoveState::Importing, Some("o.p2.1")));
        let gone = tick(&f.to, cut + ARCHIVE_KEEP_MS).expect("its expiry");
        assert_eq!(gone.effects, vec![Effect::Sweep { prefix: "p2/o/1/".into() }]);
        assert_eq!((gone.to.state, gone.to.deadline()), (MoveState::Held, None));
        assert!(matches!(retry(&gone.to, cut + ARCHIVE_KEEP_MS + 1), Err(Refused::Unfit(_))), "nothing left to import");
    }

    /// Past the cutoff, the alarm wakes for it only while there is room to
    /// archive: with every slot taken, an archiving move's own deadline
    /// wakes it, not the past cutoff again at once.
    /// A phase 2 stage goes at its expiry alone, however its move ends: a
    /// good import, or staff's retire or withdrawal after a failed one.
    #[test]
    fn a_phase_two_stage_is_kept_to_its_expiry_however_its_move_ends() {
        let cut = T0;
        let expiry = cut + ARCHIVE_KEEP_MS;
        let a = cutoff(&Move::listed("o", "u", true, T0), cut, cut).unwrap().to;
        let s = to(report(&a, &report_on(&a, 1, "stopped"), cut + 1));
        let st = to(report(&restarted(&s), &report_on(&s, 2, staged_event()), cut + 2));
        let ar = to(report(&st, &report_on(&st, 1, "retired"), cut + 3));
        let late = to(yes(&ar, AGENT, cut + 10));
        // done well: retired as migrated, its stage kept
        let d = done(&late, AGENT, "o.p2.1", &ok(), cut + 11).unwrap().unwrap();
        assert_eq!((d.effects.as_slice(), d.to.staged_ms), (&[][..], Some(cut + 2)), "kept after a good import");
        let m = to(report(&d.to, &report_on(&d.to, 1, "retired"), cut + 12));
        assert_eq!((m.state, m.deadline()), (MoveState::Moved, Some(expiry)), "moved, its stage kept to its expiry");
        assert_eq!(tick(&m, expiry - 1), None);
        let gone = tick(&m, expiry).expect("its expiry");
        assert_eq!(gone.effects, vec![Effect::Sweep { prefix: "p2/o/1/".into() }]);
        assert_eq!((gone.to.state, gone.to.deadline()), (MoveState::Moved, None));
        // a failed import held: staff's retire or withdrawal keeps it too
        let failed = StageDone { ok: false, report_sha256: None, why: Some("x".into()) };
        let held = to(done(&late, AGENT, "o.p2.1", &failed, cut + 11));
        let retired = retire(&held, cut + 12).unwrap().unwrap();
        assert_eq!((retired.effects.as_slice(), retired.to.staged_ms), (&[][..], Some(cut + 2)), "kept after staff's retire");
        let withdrawn = withdraw(&held, WHY, cut + 12).unwrap().unwrap();
        assert_eq!((withdrawn.effects.as_slice(), withdrawn.to.deadline()), (&[][..], Some(expiry)), "kept after staff's withdrawal");
        assert_eq!(tick(&withdrawn.to, expiry).expect("its expiry").effects, vec![Effect::Sweep { prefix: "p2/o/1/".into() }]);
        // and staff's include leaves it so: its old agent is retired, its
        // stage the only copy (an offer's Yes would stage it afresh)
        assert_eq!(include(&restarted(&withdrawn.to), true, cut + 13), Err(Refused::Begun));
    }

    /// A move withdrawn after its person said Yes (held), or once the cutoff
    /// took it, is not offered again by `include`: its old agent may be
    /// stopped or retired, and a Yes would stage it afresh.
    #[test]
    fn include_refuses_a_move_whose_old_agent_was_stopped() {
        let held = tick(&to(yes(&open_move(), AGENT, T0)), T0 + STOP_MS).unwrap().to;
        let w = to(withdraw(&held, WHY, T0 + 1));
        assert_eq!(include(&restarted(&w), true, T0 + 2), Err(Refused::Begun));
        let cut = T0 + 10;
        let archiving = cutoff(&open_move(), cut, cut).unwrap().to;
        // never stopped, attempt after attempt: held, archive_failed
        let mut held_archive = archiving;
        while held_archive.state != MoveState::Held {
            held_archive = tick(&held_archive, held_archive.deadline().unwrap()).unwrap().to;
        }
        let at = held_archive.since_ms;
        let w = to(withdraw(&held_archive, WHY, at + 1));
        assert_eq!(include(&w, false, at + 2), Err(Refused::Begun));
    }

    /// Each routine a stage's imports send has a key of its own: the once
    /// more, and a retry's after it was held, never repeat an earlier key
    /// (the agent's fragment appends a key once, so a repeat never reached
    /// it). Each try of one delivery reads the same key.
    #[test]
    fn a_retried_import_sends_its_routines_under_fresh_keys() {
        let cut = T0;
        let a = cutoff(&Move::listed("o", "u", true, T0), cut, cut).unwrap().to;
        let s = to(report(&a, &report_on(&a, 1, "stopped"), cut + 1));
        let st = to(report(&restarted(&s), &report_on(&s, 2, staged_event()), cut + 2));
        let ar = to(report(&st, &report_on(&st, 1, "retired"), cut + 3));
        let first = to(yes(&ar, AGENT, cut + 10));
        let once_more = tick(&first, first.deadline().unwrap()).expect("due").to;
        let held = tick(&once_more, once_more.deadline().unwrap()).expect("due").to;
        assert_eq!(held.reason, Some(HeldReason::NotOpened));
        let retried = retry(&held, held.since_ms + 1).unwrap().unwrap();
        assert_eq!(retried.effects, vec![Effect::Routine { agent: AGENT.into(), stage: "o.p2.1".into() }]);
        let again = tick(&retried.to, retried.to.deadline().unwrap()).expect("due").to;
        let keys = [&first, &once_more, &retried.to, &again].map(|m| m.routine_key().expect("importing"));
        assert_eq!(keys.iter().collect::<std::collections::HashSet<_>>().len(), 4, "{keys:?}");
        assert_eq!(restarted(&retried.to).routine_key(), retried.to.routine_key(), "one delivery, one key");
    }

    #[test]
    fn past_the_cutoff_with_no_room_the_alarm_waits_for_a_deadline() {
        let (cut, now) = (T0, T0 + 5);
        assert_eq!(next_wake(Some(cut + 99), Some(cut + 10), 1, 0, cut), Some(cut + 10), "before it: the cutoff");
        assert_eq!(next_wake(None, Some(cut), 3, ARCHIVING_MAX - 1, now), Some(cut), "past it, with room: at once");
        assert_eq!(next_wake(Some(now + 60_000), Some(cut), 3, ARCHIVING_MAX, now), Some(now + 60_000), "no room: the next deadline");
        assert_eq!(next_wake(None, Some(cut), 3, ARCHIVING_MAX, now), None, "no room, no deadline: nothing");
        assert_eq!(next_wake(None, Some(cut + 10), 0, 0, cut), None, "nothing unasked: no cutoff");
    }

    /// An archive staged, whose retirement's word never came, is held; staff's
    /// retry asks for the retirement again, its stage kept (the mover may
    /// have retired the old agent and lost its receipt: the stage is the
    /// only copy then), not a new stage of an old agent that may be gone.
    #[test]
    fn a_held_archive_retirement_is_retried_with_its_stage_kept() {
        let cut = T0;
        let a = cutoff(&Move::listed("o", "u", true, T0), cut, cut).unwrap().to;
        let s = to(report(&a, &report_on(&a, 1, "stopped"), cut + 1));
        let st = to(report(&restarted(&s), &report_on(&s, 2, staged_event()), cut + 2));
        let held = tick(&st, st.deadline().unwrap()).expect("its retirement's day").to;
        assert_eq!((held.state, held.reason, held.staged_ms), (MoveState::Held, Some(HeldReason::ArchiveFailed), Some(cut + 2)));
        let r = retry(&held, cut + 10).unwrap().unwrap();
        assert_eq!(r.effects, vec![], "its stage is kept");
        assert_eq!((r.to.state, r.to.attempt, r.to.command, r.to.staged_ms), (MoveState::Archiving, 1, Some(Act::Retire(Mark::Archived)), Some(cut + 2)));
        assert_eq!(r.to.deadline(), Some(cut + 10 + RETIRE_MS), "a day for its word, from the retry");
        // the mover's receipt, again or late, archives it
        let ar = to(report(&r.to, &report_on(&r.to, 1, "retired"), cut + 11));
        assert_eq!(ar.state, MoveState::Archived);
    }

    #[test]
    fn a_failing_archive_is_tried_again_then_held() {
        let cut = T0;
        let a = cutoff(&Move::listed("o", "u", true, T0), cut, cut).unwrap().to;
        let again = tick(&a, a.deadline().unwrap()).expect("due");
        assert_eq!((again.to.state, again.to.attempt, again.to.phase), (MoveState::Archiving, 2, Phase::P2));
        let mut m = again.to;
        m.attempt = ATTEMPTS_MAX;
        let held = tick(&m, m.deadline().unwrap()).expect("due").to;
        assert_eq!((held.state, held.reason), (MoveState::Held, Some(HeldReason::ArchiveFailed)));
        let retried = to(retry(&held, cut + 1));
        assert_eq!((retried.state, retried.phase, retried.attempt), (MoveState::Archiving, Phase::P2, ATTEMPTS_MAX + 1));
    }

    #[test]
    fn a_move_from_an_older_row_reads_with_its_defaults() {
        let row = serde_json::json!({ "old": "o", "workos_id": "u", "state": "open", "phase": "p1", "attempt": 0, "since_ms": 5 });
        assert_eq!(Move::read(&row), Ok(Move::listed("o", "u", true, 5)));
        for bad in [serde_json::json!({ "workos_id": "u", "state": "open", "phase": "p1", "since_ms": 5 }), serde_json::json!({ "old": "o", "workos_id": "u", "state": "lost", "phase": "p1", "since_ms": 5 }), serde_json::json!({ "old": "o", "workos_id": "u", "state": "open", "phase": "p1", "since_ms": 5, "command": "explode" })] {
            assert!(Move::read(&bad).is_err(), "{bad}");
        }
    }

    /// A move's command, as the mailbox answers it, for each act; and a
    /// report lacking a field its event needs is refused, changing nothing.
    #[test]
    fn commands_are_flat_and_reports_need_their_fields() {
        let mut m = to(yes(&open_move(), AGENT, T0));
        let c = m.command().unwrap();
        assert_eq!((c.id.as_str(), c.kind, c.stage.as_deref(), c.mark), ("prj_1:1:stop_and_stage", CommandKind::StopAndStage, Some("prj_1.p1.1"), None));
        m.command = Some(Act::Retire(Mark::Archived));
        let c = restarted(&m).command().unwrap();
        assert_eq!((c.id.as_str(), c.kind, c.phase, c.stage, c.mark), ("prj_1:1:retire_archived", CommandKind::Retire, None, None, Some(Mark::Archived)));
        m.command = Some(Act::Restart);
        assert_eq!(restarted(&m).command().map(|c| (c.kind, c.mark)), Some((CommandKind::Restart, None)));
        m.command = None;
        assert_eq!(m.command(), None);
        // a staged report needs its manifest's hash, bytes and files
        let s = staging();
        let mut r = report_on(&s, 2, "staged");
        r.manifest_sha256 = None;
        assert!(matches!(report(&s, &r, T0), Err(Refused::Unfit(_))));
        let mut r = report_on(&s, 2, "progress");
        r.files = None;
        assert!(matches!(report(&s, &r, T0), Err(Refused::Unfit(_))));
        let mut r = report_on(&s, 2, "failed");
        r.why = None;
        assert!(matches!(report(&s, &r, T0), Err(Refused::Unfit(_))));
    }

    /// The mover uploads only the stage it was told to, while it stages.
    #[test]
    fn the_mover_uploads_only_its_commanded_stage_while_it_stages() {
        let asked = to(yes(&open_move(), AGENT, T0));
        assert_eq!(uploadable(&asked, "prj_1.p1.1"), Err(Refused::NotNow(MoveState::Asked)), "not stopped yet");
        let s = staging();
        assert_eq!(uploadable(&s, "prj_1.p1.1"), Ok(()));
        assert_eq!(uploadable(&restarted(&s), "prj_1.p1.1"), Ok(()), "restart");
        for other in ["prj_1.p1.2", "prj_2.p1.1", "prj_1.p2.1", ""] {
            assert_eq!(uploadable(&s, other), Err(Refused::NotYours), "{other}");
        }
        let i = importing();
        assert_eq!(uploadable(&i, "prj_1.p1.1"), Err(Refused::NotYours), "staged: its command is done");
        assert_eq!(uploadable(&open_move(), "prj_1.p1.1"), Err(Refused::NotYours), "no command at all");
        // phase 2: once stopped
        let a = cutoff(&Move::listed("o", "u", false, T0), T0, T0).unwrap().to;
        assert_eq!(uploadable(&a, "o.p2.1"), Err(Refused::NotNow(MoveState::Archiving)));
        let st = to(report(&a, &report_on(&a, 1, "stopped"), T0 + 1));
        assert_eq!(uploadable(&st, "o.p2.1"), Ok(()));
    }

    /// Goal: an open move asks its person in the server's words, alone over
    /// everything (must-answer), with its three answers: Yes opens the page,
    /// Not now and Never post to the person's own routes; its picture is
    /// read through the platform, and a nameless agent is still asked.
    #[test]
    fn an_offer_asks_in_words_with_its_answers() {
        let n = offer("prj_1", " Ada ", true, "https://chat.test/move");
        assert_eq!((n.id.as_str(), n.kind.as_str(), n.level, n.must_answer), ("move:prj_1", "move_offer", NoticeLevel::Info, true));
        assert_eq!((n.title.as_str(), n.text.as_deref()), ("Bring Ada over from finite.computer?", Some("You'll get a month free.")));
        let acts: Vec<(&str, Option<&str>, Option<&str>)> = n.actions.iter().map(|a| (a.label.as_str(), a.open.as_deref(), a.post.as_deref())).collect();
        assert_eq!(
            acts,
            vec![("Yes", Some("https://chat.test/move"), None), ("Not now", None, Some("/api/migrations/offers/prj_1/later")), ("Never", None, Some("/api/migrations/offers/prj_1/never"))]
        );
        assert!(n.actions.iter().all(|a| a.body.is_none()), "each posts {{}}");
        assert_eq!(n.image.as_deref(), Some("/api/migrations/offers/prj_1/picture"));
        let bare = offer("prj_2", "  ", false, "https://chat.test/move");
        assert_eq!((bare.title.as_str(), bare.image), ("Bring your agent over from finite.computer?", None));
    }

    /// Goal: `/move` says where each move is in plain words, from its state
    /// alone: asked, stopping, copying, importing, done, or paused for staff.
    #[test]
    fn where_a_move_is_in_words() {
        let open = open_move();
        assert_eq!(said(&open, "Ada"), "Ada is on finite.computer.");
        assert_eq!(said(&to(never(&open, T0)), "Ada"), "You said never.");
        let asked = to(yes(&open, AGENT, T0));
        assert_eq!(said(&asked, "Ada"), "Stopping Ada on finite.computer…");
        assert_eq!(said(&staging(), "Ada"), "Copying Ada…");
        assert_eq!(said(&importing(), " "), "Bringing your agent over…");
        let held = tick(&asked, T0 + STOP_MS).expect("held").to;
        assert_eq!((held.state, said(&held, "Ada").as_str()), (MoveState::Held, "Paused: someone from Finite will look at it and be in touch."));
        assert_eq!(said(&restarted(&held), "Ada"), said(&held, "Ada"), "restart");
    }

    /// Goal: a person's Yes hands every secret its old agent has to their
    /// Vault (F11), only for that command, while it stops or stages, and
    /// each into an entry bound to the new agent toward its kind's hosts.
    #[test]
    fn a_yes_hands_off_the_old_agents_secrets() {
        use fragment_proto::migrations::HandoffItem;
        let all = Has { telegram: true, openrouter: true, nostr: true };
        let some = Has { telegram: true, openrouter: false, nostr: false };
        let asked = to(yes(&open_move(), AGENT, T0));
        // valid: every kind it has, on its command, while asked or staging
        assert_eq!(asked.secrets(&all), SecretKind::ALL.to_vec());
        assert_eq!(asked.secrets(&some), vec![SecretKind::TelegramBotToken]);
        let cmd = asked.command_id().unwrap();
        assert_eq!(handoff(&asked, &cmd, &all, &SecretKind::ALL), Ok(()));
        let s = staging();
        assert_eq!(handoff(&restarted(&s), &s.command_id().unwrap(), &some, &[SecretKind::TelegramBotToken]), Ok(()), "staging, restarted");
        // invalid: a kind it has not, another command, a move past staging
        assert_eq!(handoff(&s, &s.command_id().unwrap(), &some, &[SecretKind::OpenrouterKey]), Err(Refused::Unfit("a hand-off names only the secrets its command asks for")));
        assert_eq!(handoff(&s, "prj_1:2:stop_and_stage", &all, &[SecretKind::NostrNsec]), Err(Refused::Stale));
        let i = importing();
        assert_eq!(handoff(&i, &cmd, &all, &[SecretKind::NostrNsec]), Err(Refused::Stale), "staged: its command is done");
        // none without a Yes: phase 2's stop_and_stage, an unasked move, staff's restart
        let p2 = cutoff(&Move::listed("o", "u", false, T0), T0, T0).unwrap().to;
        assert_eq!(p2.secrets(&all), vec![]);
        assert!(handoff(&p2, &p2.command_id().unwrap(), &all, &[SecretKind::NostrNsec]).is_err());
        assert_eq!(open_move().secrets(&all), vec![]);
        // its bounds: each kind once, 1 to 3, a value 1 to 4096 bytes
        let item = |kind, value: &str| HandoffItem { kind, value: value.into() };
        let h = |items| Handoff { command: cmd.clone(), items };
        assert_eq!(handoff_kinds(&h(vec![item(SecretKind::NostrNsec, "x"), item(SecretKind::TelegramBotToken, "y")])), Ok(vec![SecretKind::NostrNsec, SecretKind::TelegramBotToken]));
        assert!(handoff_kinds(&h(vec![])).is_err());
        assert!(handoff_kinds(&h(vec![item(SecretKind::NostrNsec, "x"), item(SecretKind::NostrNsec, "y")])).is_err());
        assert!(handoff_kinds(&h(vec![item(SecretKind::OpenrouterKey, "")])).is_err());
        assert!(handoff_kinds(&h(vec![item(SecretKind::OpenrouterKey, &"k".repeat(HANDOFF_VALUE_MAX_BYTES + 1))])).is_err());
        assert!(handoff_kinds(&Handoff { command: "c".repeat(TEXT_MAX_BYTES + 1), items: vec![item(SecretKind::NostrNsec, "x")] }).is_err());
        assert!(handoff_kinds(&Handoff { command: format!("{cmd}\nnostr_nsec"), items: vec![item(SecretKind::NostrNsec, "x")] }).is_err(), "one line");
    }

    #[test]
    fn each_kind_is_a_vault_entry_of_the_new_agent() {
        use crate::catalog::Catalog;
        let (name, put) = vault_secret(SecretKind::TelegramBotToken, "kestrel--x7k2", "123:abc").unwrap();
        assert_eq!((name.as_str(), put.hosts.as_slice(), put.agents.as_slice(), put.value.as_str()), ("telegram-kestrel--x7k2", &["api.telegram.org".to_string()][..], &["kestrel--x7k2".to_string()][..], "123:abc"));
        let (name, put) = vault_secret(SecretKind::OpenrouterKey, "kestrel--x7k2", "sk-or-1").unwrap();
        assert_eq!((name.as_str(), put.hosts.as_slice()), ("openrouter-kestrel--x7k2", &["openrouter.ai".to_string()][..]));
        // NIP-19's example key: kept as hex, to sign with, sent nowhere
        let nsec = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";
        let hex = "67dea2ed018072d675f5415ecfaed7d2597555e202d85b3d65ea4e58d2d92ffa";
        let (name, put) = vault_secret(SecretKind::NostrNsec, "kestrel--x7k2", nsec).unwrap();
        assert_eq!((name.as_str(), put.value.as_str(), put.hosts.len(), put.place.sign.as_deref()), ("nostr-kestrel--x7k2", hex, 0, Some("nip98")));
        assert_eq!(vault_secret(SecretKind::NostrNsec, "kestrel--x7k2", hex).unwrap().1.value, hex, "hex too");
        assert_eq!(vault_secret(SecretKind::NostrNsec, "kestrel--x7k2", "nsec1nope"), None, "not a key");
        // a long label is cut short to fit a name; every entry passes the Vault's rules
        let long = format!("{}--x7k2", "a".repeat(20) + "-b");
        let catalog = Catalog::default();
        for kind in SecretKind::ALL {
            let value = if kind == SecretKind::NostrNsec { nsec } else { "tok-123" };
            for fragment in ["kestrel--x7k2", long.as_str()] {
                let (name, put) = vault_secret(kind, fragment, value).unwrap();
                assert!(name.len() <= 32 && name.ends_with("--x7k2"), "{name}");
                crate::vault::check(&name, &put, &catalog).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
    }
}
