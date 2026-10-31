//! The bridge's core, as a pure state machine: records and runtime events
//! in, posts and runtime commands out. Nothing here does I/O or reads the
//! clock; the driver (driver.rs) feeds it, persists its state whenever a
//! step says so, and only then carries out the step's effects ("validate,
//! persist, then interpret", the engineering style).
//!
//! What it keeps (lesson 1, authority): a cursor per followed channel and
//! the turns it admitted that have not ended. The chat channel owns what was
//! said; the chat's `work` channel owns which turns have started (the
//! journal); the runtime owns the in-flight turn; the bridge only
//! translates. Its state is a cache: lost, or restored from any earlier
//! save, it costs reads, never a second run (docs/computers.md,
//! Durability, P1).
//!
//! Its rules, in one place:
//! - A record is admitted once in a life: the cursor of its `(agent,
//!   fragment, channel)` passes it before anything it causes is done, so a
//!   catch-up after a reconnect starts nothing twice (lesson 2).
//! - A turn runs only in the life that claimed it. Each bridge process is a
//!   life, with 128 random bits of its own that are never written to
//!   `/data`. A turn's claim is its `turn.start` on `work`, naming the
//!   life; the runtime is given the turn only once the platform answers
//!   the claim as this life's (appended, or a replay of this life's own
//!   retry). A 409 is another life's claim: never run here, the turn is
//!   ended as lost (a 409 itself when that life ended it) and forgotten. No
//!   answer starts nothing: the turn stays queued and is claimed again. So
//!   a cursor a rollback sent back reads a record again and runs nothing
//!   twice (one life per turn, at most once).
//! - A life's first step ends every turn its state holds that is not
//!   queued: an earlier life claimed it, and that life is gone. The agent's
//!   next turn in that chat is told what was cut (P5): its note is built
//!   from the chat's journal alone, before its claim (note.rs), so it is
//!   said once, and the same in any life.
//! - A turn another life claimed (409) ran after this life's `/data` was
//!   saved, so its runtime remembers none of it: the agent's next turn in
//!   that chat is told what those turns were (`TurnStart::forgotten`,
//!   note.rs "forgotten"), once in this life.
//! - A turn is claimed only while the runtime can take it (`Connected`),
//!   so a turn waits, unclaimed, for a runtime that is still starting.
//! - Every turn gets both records: a turn refused, or stopped while it
//!   waited, posts its `turn.start` and then its `turn.end`.
//! - A turn leaves the state only once its last records are answered: an
//!   ended turn keeps the records it owes `work` (its end; and its start,
//!   when it never ran) until the lane is done with them, and a life that
//!   ends first leaves them to the next, which posts them again under the
//!   same ids and bodies (a replay when they had landed). So a crash
//!   between the state and the platform leaves no turn open.
//! - One turn of an agent runs in a chat at a time; the rest wait in order.
//! - In a chat with several agents, a message is for the agents it names
//!   (`to`, else `@mentions` of this computer's agents), else for the lead,
//!   the first agent added (decision 8). An agent's reply is for another
//!   agent only when it names it, at most `HOPS_MAX` hand-offs deep.
//! - The hop is the answering bridge's to count (`hop_of`), never fewer
//!   than the record claims: a record by an agent of this computer is one
//!   hop past the turn that agent is in (here, or the one just ended here
//!   that its reply names; and its deepest elsewhere; in none, the last hop
//!   allowed), so a post made
//!   around the bridge (the CLI, the API) resets nothing. A person's agents
//!   all run on one computer (decision 13), so every hand-off between them
//!   is counted here; another computer's agent is held by its claim.
//! - A chat's agents start at most `AGENT_TURNS_PER_CHAT_MAX` turns of each
//!   other in `AGENT_TURNS_WINDOW_MS` (the causing records' times, kept in
//!   the state); past it a hand-off is refused, its end saying why.
//! - An agent's `tasks` hears only its own fragment (its cron, the
//!   platform's `joined`) and its owner: another agent acting for the owner
//!   starts no routine there.
//! - Only a turn's asker stops it; only an agent's owner answers its
//!   prompts, the first answer wins, and an unanswered prompt expires
//!   (decision 42).
//! - Only an agent's owner commands its runtime, and only with its menu
//!   (`MenuItem`), each carried as the menu says (`How`): a turn of its
//!   own, the chat's turns stopped, a message, or said beside the running
//!   turn (`Command::Aside`, no turn). The menu is posted in a chat as the
//!   agent first runs there in a life.
//! - A turn that asks its asker something in words (`Asked`) says so
//!   (`turn.asked`) and takes their next message in the chat as its answer
//!   (`Tell`), never as a turn behind it; a turn waiting on a card that
//!   takes words (a clarify's "Other") takes its owner's next message as
//!   that option answered in those words, the card closed. Words, files or
//!   both: an answer's files go with it. It is a running turn of this
//!   life: a restart ends it as lost, and that message, read by the next
//!   life, is a turn of its own.
//! - Wakefulness follows docs/bridge.md, including runtime activity.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::limits;
use crate::records::{self, AttachmentRef, Cause, Closed, CommandSaid, Message, NoticeCategory, Outcome, PromptOption, Record, Said, Task};
use crate::runtime::{Agent, Attached, Command, Event, How, LocalFile, MenuItem, TurnStart};

/// The state file's formats. 2: one life per turn (no `handed` phase). 3:
/// a turn holding an ask's return (`Turn::ret`) and the answer it owes,
/// which a bridge before asks cannot read: it sets such a state aside
/// (driver.rs, `load`), as it does any format not its own, and the journal
/// says which turns ran. A state holding no return is written as 2, so a
/// rollback past asks keeps every other state. 4: a turn that continues
/// a cut one, or one to be continued (`Turn::continues`,
/// `Turn::continue_after`), which a bridge before continuations would
/// refuse (a continuation's id is not its cause's): it sets such a state
/// aside the same way.
pub const STATE_VERSION: u32 = 4;
pub const STATE_VERSIONS: [u32; 3] = [2, 3, STATE_VERSION];

/// The format a state is written as (`STATE_VERSION`'s).
fn version_of(state: &State) -> u32 {
    if state.turns.values().any(|t| t.continues.is_some() || t.continue_after) {
        STATE_VERSION
    } else if state.turns.values().any(|t| t.ret.is_some()) {
        3
    } else {
        2
    }
}

/// A turn a restart cut may be continued (`Engine::continue_turn`): never
/// a continuation (one continuation, ending visibly), one its asker stopped
/// (a Stop this life accepted: the work is cancelled, never picked up
/// again), an ask's turn (its asker is answered `lost`) or a command.
fn continued(t: &Turn) -> bool {
    t.continues.is_none() && !t.stop_requested && t.ret.is_none() && !t.command
}

/// Why an agent's hand-off is refused a turn past the hops (an ask, which
/// is answered so; any other is let go).
pub fn refused_deep() -> String {
    format!("agents handed this on {} times without a person, the most they may", limits::HOPS_MAX)
}

/// What an ask's turn that asks back in words is told at once: its asker
/// is an agent, which cannot answer it (`Event::Asked`). Its `seq` names
/// no record (the bridge's own words): Relay's message id for it is the
/// turn's and this.
pub const ASKED_BACK: &str = "No answer is coming: you were asked by another agent, which cannot answer questions here. Answer as best you can with what you have, and say what you would need.";
pub const ASKED_BACK_SEQ: u64 = u64::MAX;

/// Why a turn an earlier life claimed is ended.
pub const LOST: &str = "lost when the computer restarted";

/// Why a message is refused a turn: too many wait for its agent in its chat,
/// or the computer holds as many turns as it may. A refused turn posts its
/// start and its end and never runs (the note's rule passes over it:
/// note.rs).
pub const REFUSED_QUEUED: &str = "too many messages are waiting for this agent; send it again once it answers";
pub const REFUSED_BUSY: &str = "this computer is too busy right now";

/// Who says a cut turn's continuation (`Engine::continue_turn`): the
/// computer, never its asker, whose words it is not.
pub const CONTINUE_ASKER: &str = "your computer";

/// What a cut turn's continuation says, after its note (what was cut, and
/// each call that may or may not have run).
pub const CONTINUE_TEXT: &str = "I restarted while you were working on the turn above, so it stopped partway. Pick it up again now. First check what it already did; don't redo anything that may already have taken effect. If it was finished, say so briefly.";

/// What the chat is shown as a cut turn's continuation starts.
pub const CONTINUE_NOTICE: &str = "Picking up where it left off after a restart";

/// Why an agent's turn is refused: its `agent.json` names a runtime this
/// computer's bridge does not run (its image carries none by that name).
pub fn refused_runtime(runtime: &str) -> String {
    if crate::runtime::valid_name(runtime) {
        format!("this computer doesn't run {runtime} agents")
    } else {
        "this computer doesn't run this agent's runtime".to_string()
    }
}

/// Why an agent's hand-off is refused a turn: the chat's agents started as
/// many turns of each other as they may in the window
/// (`limits::AGENT_TURNS_PER_CHAT_MAX`).
pub fn refused_budget() -> String {
    format!(
        "agents in this chat started {} turns of each other in {} minutes, the most they may; ask again in a few minutes",
        limits::AGENT_TURNS_PER_CHAT_MAX,
        limits::AGENT_TURNS_WINDOW_MS / 60_000
    )
}

/// What the bridge keeps across restarts (`/data/bridge/state.json`): a
/// cache of where to read from and what it has in hand, never the
/// authority on which turns have run (the journal is).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    /// How many times this state was loaded (a restored one counts from its
    /// save's number again): for the log.
    pub boot: u64,
    /// The last seq handled, per `agent|fragment|channel`.
    pub cursors: BTreeMap<String, u64>,
    /// The turns admitted and not over, by id: queued, running, waiting,
    /// or ended and still owing `work` their last records.
    pub turns: BTreeMap<String, Turn>,
    /// Admissions so far: a turn's place in its chat's queue.
    pub admitted: u64,
    /// Messages the runtime said on its own so far (their turns' ids, with
    /// the life).
    pub said: u64,
    /// Per chat, the times (the causing records' `at`, ms, ascending) of
    /// the turns its agents started of each other here lately: its budget
    /// (`limits::AGENT_TURNS_PER_CHAT_MAX` in `AGENT_TURNS_WINDOW_MS`).
    /// Kept, so a restart spends none of it again. A state without it (an
    /// earlier bridge's) starts each chat's count afresh.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_turns: BTreeMap<String, Vec<i64>>,
}

impl Default for State {
    fn default() -> State {
        State { version: 2, boot: 0, cursors: BTreeMap::new(), turns: BTreeMap::new(), admitted: 0, said: 0, agent_turns: BTreeMap::new() }
    }
}

/// Where a turn is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Admitted, and not claimed as this life's: another turn of its agent
    /// runs in its chat, its runtime cannot take it yet, or its claim is
    /// unanswered (in flight, held, or lost). Any life may claim it.
    Queued,
    /// Its claim was answered as this life's, and the runtime has it (a
    /// restart ends it: one life per turn).
    Running,
    /// Waiting on a prompt's answer, or its expiry; eligible for idle sleep.
    Waiting,
    /// Over (run and ended, or ended without running: refused, stopped
    /// while it waited, or another life's), and its last records are owed
    /// to `work` until the platform answers them (`Turn::owed`).
    Ended,
}

/// One of a turn's last records, kept with the turn until the lane is done
/// with it: its `turn.end`, and for a turn that never ran its `turn.start`
/// too. A life that ends first leaves it to the next, which posts it again
/// at its start under the same id and body (a replay when it had landed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Owed {
    pub id: String,
    pub body: Value,
    /// Posted as this agent on its own fragment's `tasks`, not on the turn's
    /// chat's `work`: the answer to an ask (`Return`), which wakes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// Where an asked turn's answer goes (`Message::return_to`): the asking
/// agent of this computer, whose own `tasks` it is posted on, and the chat
/// its answer's turn runs in; what the asked agent is called (its name as
/// its chat showed it when asked: its agent fragment's title); and what the
/// turn said so far, which it carries (its replies but interim ones, at
/// most `ANSWER_TEXT_MAX_BYTES`), with how many files it sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Return {
    pub agent: String,
    pub chat: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub said: String,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub files: u32,
}

/// The platform's answer to a claim (`Effect::Claim`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimAnswer {
    /// Appended, or a replay of this life's own retry: this life runs it.
    /// `seq` is the claim's place on the chat's `work`, as the platform's
    /// answer names it: the journal before it says what the turn is told of
    /// the agent's turn before it there (note.rs).
    Ours { seq: Option<u64> },
    /// 409: another life's claim holds the id.
    Theirs,
    /// 403 or 404: the agent may not post on the chat's `work` (it left
    /// the chat, or its owner holds it below editor). Nothing runs, nothing
    /// can be recorded there, and the turn is dropped, as a fragment the
    /// agent is gone from drops its turns.
    Refused,
    /// No answer after the lane's tries, or never sent (the computer is
    /// held): nothing is known, so nothing starts.
    Unanswered,
}

/// A prompt a turn asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub options: Vec<String>,
    /// Its options answered in words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    pub expires_at: u64,
    pub closed: bool,
    /// Its owner's answer, held from the runtime until its closure is on
    /// `work` (`Effect::Fence`). A restart ends its turn, so never kept.
    #[serde(skip)]
    pub held: Option<Held>,
}

/// A live answer held from the runtime (`Prompt::held`): its card's
/// closure, posted again until the platform answers it, and the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub closed: Value,
    pub answer: Command,
}

/// The reply a turn is writing: shown as the chat's draft, posted once a
/// later part, a step, a prompt, or the end comes. Not kept across a
/// restart, which ends the turn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenReply {
    pub part: u32,
    pub text: String,
    pub files: Vec<LocalFile>,
}

/// A turn the bridge admitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    /// The agent fragment whose turn it is.
    pub agent: String,
    /// The chat it answers in.
    pub fragment: String,
    /// The record that started it.
    pub cause: Cause,
    pub asker: String,
    pub asker_name: String,
    pub text: String,
    pub attachments: Vec<AttachmentRef>,
    pub routine: bool,
    /// Agent hand-offs that led to it.
    pub hop: u32,
    /// Its admission's number: the order turns of a chat run in.
    pub order: u64,
    pub phase: Phase,
    /// When the runtime was last heard about it (or it was handed).
    pub last_ms: u64,
    pub steps: u32,
    /// Replies posted so far (their ids' numbers), and the highest part
    /// among them.
    pub replies: u32,
    pub last_part: u32,
    /// Of those, the interim ones (words before a tool call), which spend
    /// no part of `REPLIES_PER_TURN_MAX`. A state from before holds none.
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub interim: u32,
    /// Tool calls it asked for (`turn.call`), recorded or not: they share
    /// `STEPS_PER_TURN_MAX`'s bound. A state from before holds none.
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub calls: u32,
    pub prompts: Vec<Prompt>,
    pub stop_requested: bool,
    /// What an ended turn still owes `work`, in the order posted.
    pub owed: Vec<Owed>,
    /// It asked its asker something to answer in words (`Event::Asked`):
    /// their next message in the chat is its answer, not a turn. Only a
    /// turn this life runs asks: a restart ends it as lost, as it ends every
    /// turn not queued, and a turn's end clears this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asking: bool,
    /// Its text is a command of the runtime's menu, said to it as it is
    /// (`TurnStart::command`). A state from before commands holds none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub command: bool,
    /// The seq on `chat` of the message its message quotes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// What it is told before its message, for a late card answer's turn:
    /// the card's turn (cards.rs, `Fresh::note`; `TurnStart::note`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// It answers an ask: its end owes the answer too (`Return`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ret: Option<Return>,
    /// It continues the turn named, which a restart cut (`continue_turn`):
    /// one continuation, and one that is cut itself is never continued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<String>,
    /// A life's start ended it as lost, and it is continued once its end
    /// lands as that life's own (`posted`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continue_after: bool,
    #[serde(skip)]
    pub open: Option<OpenReply>,
}

/// What a turn is asked (`Engine::admit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask<'a> {
    /// A message (a person's, or an agent's hand-off).
    Message,
    /// A routine (its agent's cron), asked as its owner.
    Routine,
    /// A command of the runtime's menu, its text said as it is.
    Command,
    /// A command its menu refuses with these words: both its records, and
    /// the end says why; the runtime never hears it.
    Refused(&'a str),
}

impl Turn {
    /// This life's claim was answered, and the runtime has it.
    fn active(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Waiting)
    }
}

/// Who is in a chat, as the driver read it (members and `__people`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatView {
    /// The agents among its members, in the order they were added: the
    /// first is the lead.
    pub agents: Vec<String>,
    /// The people among its members.
    pub people: Vec<String>,
    /// What to call its writers, by identity.
    pub names: BTreeMap<String, String>,
}

impl ChatView {
    pub fn lead(&self) -> Option<&str> {
        self.agents.first().map(String::as_str)
    }
}

/// What the engine is told.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The computer's agents, as `GET /api/computer` lists them now.
    Agents(Vec<Agent>),
    /// A record of a followed channel, read as `agent`, with the chat's view
    /// (a `chat` record's admission needs it; a `tasks` record's does not).
    /// A record from before `since` (ms; when the agent joined the chat) is
    /// history: its cursor passes it, and it starts nothing.
    Record { agent: String, fragment: String, record: Record, view: Option<ChatView>, since: i64 },
    /// An owner's answer to a card of the agent's that no running turn
    /// held, with the chat's journal read for it (`work` back to the card's
    /// turn, `chat` back to the card, the turn's cause), or why it could
    /// not be, finally (`Engine::card_record`).
    CardRecord { agent: String, fragment: String, record: Record, view: Option<ChatView>, since: i64, history: Result<crate::history::Journal, crate::history::Unavailable> },
    /// What a runtime said, by its name (`with_runtime`).
    Runtime { runtime: String, event: Event },
    /// The platform's answer to a live answer's closure (`Effect::Fence`).
    Fenced { turn: String, prompt: String, answer: ClaimAnswer },
    /// The platform's answer to a turn's claim.
    Claimed { turn: String, answer: ClaimAnswer },
    /// The lane is done with an owed record (`Effect::Owed`): answered
    /// (appended, a replay, or refused), or given up after its tries.
    /// `kept`: the platform holds its body as this life's (appended, or a
    /// replay of this body), not another life's (409) nor refused.
    Posted { turn: String, id: String, kept: bool },
    /// Time passed: expiries, quiet turns, and claims to try again.
    Tick,
    /// The agent is no longer in the fragment (403 or 404 there).
    Gone { agent: String, fragment: String },
    /// The agents whose SimpleX link is bound now (simplex/run.rs), each
    /// with its SimpleX chat once found: what each says in that chat goes to
    /// its owner's phone (`mirror`). Their relayed records are honored with
    /// or without it.
    Relaying(BTreeMap<String, Option<String>>),
}

/// What the engine asks the driver to do, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A record, as `agent`; `files` are uploaded as the fragment's blobs and
    /// listed in the body's `attachments` first.
    Post { agent: String, fragment: String, channel: &'static str, id: String, body: Value, files: Vec<LocalFile> },
    /// A turn's claim: its `turn.start` on `work` as this life, posted in
    /// its fragment's order unless the computer is held, and answered back
    /// as `Input::Claimed`. The runtime hears of the turn only after.
    Claim { agent: String, fragment: String, turn: String, id: String, body: Value },
    /// A live card answer's closure on `work`, posted in its fragment's
    /// order and answered back as `Input::Fenced`: the runtime has the
    /// answer only once the journal holds it, so a life rolled back to
    /// before the answer finds it answered live, and rebuilds nothing
    /// (cards.rs).
    Fence { agent: String, fragment: String, turn: String, prompt: String, id: String, body: Value },
    /// One of a turn's last records (on `work`: its end, and its start for a
    /// turn that never ran; on its asking agent's own `tasks`: its answer),
    /// kept in the state until the lane is done with it and says so
    /// (`Input::Posted`).
    Owed { agent: String, fragment: String, channel: &'static str, turn: String, id: String, body: Value },
    /// The chat's draft for a turn (`None` stops it).
    Draft { agent: String, fragment: String, turn: String, text: Option<String> },
    /// A command for the runtime named (`with_runtime`): the one its turn
    /// was handed to, or, beside any turn, its agent's.
    Runtime { runtime: String, command: Command },
    /// Hold the keepalive socket (true) or drop it.
    Keepalive(bool),
    /// List the agent's fragments again (it joined one: `joined`, whose
    /// members changed, so every view of it read before is stale).
    Discover { agent: String, joined: Option<String> },
}

/// One step's result.
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    pub effects: Vec<Effect>,
    /// The state changed: persist it before carrying out the effects.
    pub dirty: bool,
}

/// A state file that contradicts itself: refused, never repaired in place.
#[derive(Debug, Clone, PartialEq)]
pub struct Corrupt(pub String);

/// The engine's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub prompt_ttl_ms: u64,
    pub turn_idle_ms: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { prompt_ttl_ms: limits::PROMPT_TTL_MS_DEFAULT, turn_idle_ms: limits::TURN_IDLE_MS_MAX }
    }
}

pub struct Engine {
    state: State,
    settings: Settings,
    /// This bridge process's life: never written to the state, so no save
    /// carries it into another life.
    life: String,
    /// The runtimes the bridge runs, by the name agents give them
    /// (`with_runtime`), with each one's menu and what it says of itself.
    runtimes: BTreeMap<String, RuntimeOf>,
    /// The runtime each turn this life handed went to: its later commands go
    /// there, and its events come only from there. Never written to `/data`:
    /// a restart ends every turn an earlier life handed. At most the turns
    /// running (let go of at each tick).
    handed: BTreeMap<String, String>,
    /// Turns whose claim is posted and not answered yet: queued in the
    /// state, so a crash leaves them for the next life to claim.
    claiming: BTreeSet<String>,
    /// Live answers' closures posted and not answered yet, by turn and
    /// prompt (`Effect::Fence`): one not answered is posted again at a tick.
    fencing: BTreeSet<(String, String)>,
    agents: Vec<Agent>,
    views: HashMap<String, ChatView>,
    keepalive: bool,
    /// Turns that ran here and ended lately, oldest first (at most
    /// `ENDED_HOPS_MAX`, each for `ENDED_HOPS_MS`): a reply of one read after
    /// it was let go is one hop past it (`hop_of`). Never written to `/data`.
    ended: VecDeque<EndedTurn>,
    /// Per agent and chat, its turns there another life ran since this
    /// life's `/data` was saved (each claim answered 409), the newest at
    /// most `NOTE_FORGOTTEN_MAX`, and how many more: its runtime remembers
    /// none of them, so its next turn there is told (`TurnStart::forgotten`)
    /// and the entry goes. Never written to `/data`: a life that restores
    /// the same old `/data` reads them again, and is told again.
    forgotten: BTreeMap<(String, String), (VecDeque<String>, u32)>,
    /// Per agent and chat, its menu posted there in this life (at its first
    /// turn there: `records::menu_id`). Never written to `/data`; at most
    /// one entry per followed chat of each agent.
    published: BTreeSet<(String, String)>,
    /// The agents whose SimpleX link is bound (`Input::Relaying`), each with
    /// its SimpleX chat once found: that chat is their owner's phone
    /// (`mirror`).
    /// Never written to `/data`: the bridge reads it from each link's file
    /// as it starts.
    relaying: BTreeMap<String, Option<String>>,
    /// Per agent and chat, its turn there the runtime's notices belong to
    /// (the one running, else the last this life handed there) and how many
    /// it posted, the one used last at the back (at most
    /// `NOTICE_CHATS_MAX`). Never written to `/data`: a turn runs in one
    /// life, and a notice no turn of this life's is there for goes unposted.
    noticed: VecDeque<Noticed>,
    // A step's scratch, cleared at each step's start.
    out: Vec<Effect>,
    dirty: bool,
    now: u64,
}

/// A runtime the bridge runs: the commands it takes from its agents' owners
/// (its menu), and what it says of itself. Never saved: a new life hears it
/// again.
#[derive(Debug, Clone, PartialEq)]
struct RuntimeOf {
    menu: &'static [MenuItem],
    /// It can take turns now.
    connected: bool,
    /// Work it still owns outside its parent chat turns. A restart runs
    /// none of that process's children.
    working: bool,
}

/// The turn an agent's notices in a chat belong to, and how many it posted.
#[derive(Debug, Clone, PartialEq)]
struct Noticed {
    agent: String,
    fragment: String,
    turn: String,
    notices: u32,
}

/// A turn that ran here and ended: whose, where, how deep, and when (the
/// engine's clock).
#[derive(Debug, Clone, PartialEq)]
struct EndedTurn {
    id: String,
    agent: String,
    fragment: String,
    hop: u32,
    at: u64,
}

pub fn cursor_key(agent: &str, fragment: &str, channel: &str) -> String {
    format!("{agent}|{fragment}|{channel}")
}

/// A count that is nothing, kept out of the state file (so a state with
/// none reads as an older bridge's did).
fn u32_is_zero(n: &u32) -> bool {
    *n == 0
}

fn label(fragment: &str) -> &str {
    fragment.split('.').next().unwrap_or(fragment)
}

impl Engine {
    /// An engine over a loaded state, in the life `life` (this process's,
    /// 32 lowercase hex). It checks the state agrees with itself; then its
    /// agents (`Input::Agents`) and `recover` come before any other step.
    pub fn new(state: State, settings: Settings, life: &str) -> Result<Engine, Corrupt> {
        assert!(records::valid_life(life), "a life is 32 lowercase hex: {life}");
        check(&state)?;
        Ok(Engine {
            state,
            settings,
            life: life.to_string(),
            runtimes: BTreeMap::new(),
            handed: BTreeMap::new(),
            claiming: BTreeSet::new(),
            fencing: BTreeSet::new(),
            agents: Vec::new(),
            views: HashMap::new(),
            keepalive: false,
            ended: VecDeque::new(),
            forgotten: BTreeMap::new(),
            published: BTreeSet::new(),
            relaying: BTreeMap::new(),
            noticed: VecDeque::new(),
            out: Vec::new(),
            dirty: false,
            now: 0,
        })
    }

    /// A runtime the bridge runs, by the name agents give it
    /// (`runtime::DEFAULT` for those that name none), and the commands it
    /// takes from its agents' owners (`Runtime::menu`): the only ones a
    /// `{kind: "command"}` record carries to it, and the menu its agents'
    /// chats are offered. It takes no turn until it says it can
    /// (`Event::Connected`).
    pub fn with_runtime(mut self, name: &str, menu: &'static [MenuItem]) -> Engine {
        assert!(crate::runtime::valid_name(name), "a runtime's name: {name:?}");
        assert!(menu.iter().all(|m| records::valid_command(m.name)), "a menu names its commands");
        assert!(self.runtimes.len() < limits::RUNTIMES_MAX, "a bridge runs at most {} runtimes", limits::RUNTIMES_MAX);
        let had = self.runtimes.insert(name.to_string(), RuntimeOf { menu, connected: false, working: false });
        assert!(had.is_none(), "a runtime is run once: {name}");
        self
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn life(&self) -> &str {
        &self.life
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    /// The chat that is `agent`'s owner's phone: its SimpleX chat, while its
    /// link is bound and the chat found (`Input::Relaying`). What the agent
    /// says there goes to their SimpleX app too, whatever started its turn
    /// (docs/bridge.md, "SimpleX").
    pub fn mirror(&self, agent: &str) -> Option<&str> {
        self.relaying.get(agent)?.as_deref()
    }

    pub fn cursor(&self, agent: &str, fragment: &str, channel: &str) -> u64 {
        self.state.cursors.get(&cursor_key(agent, fragment, channel)).copied().unwrap_or(0)
    }

    pub fn keepalive(&self) -> bool {
        self.keepalive
    }

    /// The first step of a life: a new boot number; every record an ended
    /// turn still owes is posted again, under the same id and body (a
    /// replay when the life before had landed it, so the turn is let go);
    /// and every turn running or waiting is ended as lost. An earlier life
    /// claimed it (its claim was answered, or the state would hold it
    /// queued), and the runtime that had it died with that life: one life
    /// per turn, never handed again. Queued turns stay queued, for this
    /// life to claim once its runtime can take them.
    pub fn recover(&mut self, now: u64) -> Step {
        self.begin(now);
        assert!(self.claiming.is_empty(), "a life recovers before it claims");
        self.state.boot += 1;
        self.dirty = true;
        let owed: Vec<(String, String, String, Owed)> = self.state.turns.values().flat_map(|t| t.owed.iter().map(|o| (t.agent.clone(), t.fragment.clone(), t.id.clone(), o.clone()))).collect();
        for (agent, fragment, turn, o) in owed {
            crate::ev!("owed.again", { "turn": turn, "id": o.id });
            self.owe(&agent, &fragment, &turn, o);
        }
        let earlier: Vec<String> = self.state.turns.values().filter(|t| t.active()).map(|t| t.id.clone()).collect();
        for id in &earlier {
            // continued once its lost end lands as this life's (`posted`),
            // unless it waited on its person: their answer is its next turn
            let t = self.state.turns.get_mut(id).expect("listed");
            t.continue_after = continued(t) && !t.asking && t.phase != Phase::Waiting;
            self.end(id, Outcome::Error(LOST.into()), Closed::Expired);
        }
        assert!(self.state.turns.values().all(|t| matches!(t.phase, Phase::Queued | Phase::Ended)), "a recovered life holds only queued and ended turns");
        self.pump_all();
        self.finish()
    }

    /// One input, its effects, and whether to persist first.
    pub fn step(&mut self, input: Input, now: u64) -> Step {
        self.begin(now);
        match input {
            Input::Agents(agents) => self.set_agents(agents),
            Input::Record { agent, fragment, record, view, since } => self.record(&agent, &fragment, record, view, since),
            Input::CardRecord { agent, fragment, record, view, since, history } => self.card_record(&agent, &fragment, record, view, since, history),
            Input::Runtime { runtime, event } => self.runtime(&runtime, event),
            Input::Claimed { turn, answer } => self.claimed(&turn, answer),
            Input::Fenced { turn, prompt, answer } => self.fenced(&turn, &prompt, answer),
            Input::Posted { turn, id, kept } => self.posted(&turn, &id, kept),
            Input::Tick => self.tick(),
            Input::Gone { agent, fragment } => self.gone(&agent, &fragment),
            Input::Relaying(agents) => {
                assert!(agents.len() <= limits::AGENTS_MAX, "the bridge runs at most so many agents");
                self.relaying = agents;
            }
        }
        self.finish()
    }

    fn begin(&mut self, now: u64) {
        assert!(self.out.is_empty(), "a step's effects were taken");
        self.dirty = false;
        // The clock may step back (the guest's); the engine never does.
        self.now = self.now.max(now);
    }

    fn finish(&mut self) -> Step {
        let working = self.runtimes.values().any(|r| r.working);
        let busy = working || self.state.turns.values().any(|t| t.phase == Phase::Queued || (t.phase == Phase::Running && !t.asking));
        if busy != self.keepalive {
            self.keepalive = busy;
            self.out.push(Effect::Keepalive(busy));
        }
        assert!(self.state.turns.len() <= limits::TURNS_OPEN_MAX, "admission bounds the open turns");
        // the format it holds: 3 only while a turn holds a return
        let version = version_of(&self.state);
        if version != self.state.version {
            self.state.version = version;
            self.dirty = true;
        }
        for id in &self.claiming {
            assert!(self.state.turns.get(id).is_some_and(|t| t.phase == Phase::Queued), "a turn being claimed is held, queued: {id}");
        }
        debug_assert!(check(&self.state).is_ok(), "a step keeps the state whole");
        Step { effects: std::mem::take(&mut self.out), dirty: self.dirty }
    }

    fn agent(&self, fragment: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.fragment == fragment)
    }

    /// The runtime an agent runs on: the one its `agent.json` names, else
    /// `runtime::DEFAULT`. One this bridge does not run is still its name:
    /// its turns are refused (`pump`), never handed to another.
    fn runtime_of(&self, agent: &str) -> String {
        self.agent(agent).and_then(|a| a.runtime.clone()).unwrap_or_else(|| crate::runtime::DEFAULT.to_string())
    }

    /// The commands an agent's runtime takes (none: a runtime this bridge
    /// does not run).
    fn menu_of(&self, agent: &str) -> &'static [MenuItem] {
        self.runtimes.get(&self.runtime_of(agent)).map_or(&[], |r| r.menu)
    }

    /// A command about a turn, for the runtime this life handed it to. One
    /// it never handed (an earlier life's, ended at this one's start) has
    /// no runtime that knows it: nothing is said.
    fn tell_runtime(&mut self, turn: &str, command: Command) {
        match self.handed.get(turn) {
            Some(runtime) => self.out.push(Effect::Runtime { runtime: runtime.clone(), command }),
            None => crate::ev!("command.unsaid", { "turn": turn, "why": "no runtime of this life holds it" }),
        }
    }

    fn post(&mut self, agent: &str, fragment: &str, channel: &'static str, id: String, body: Value, files: Vec<LocalFile>) {
        self.out.push(Effect::Post { agent: agent.to_string(), fragment: fragment.to_string(), channel, id, body, files });
    }

    /// Posts a record the turn keeps owing until the lane is done with it:
    /// on its chat's `work` as its agent, or an answer as the agent it is for.
    fn owe(&mut self, agent: &str, fragment: &str, turn: &str, o: Owed) {
        let (agent, fragment, channel) = match &o.to {
            Some(to) => (to.as_str(), to.as_str(), records::TASKS),
            None => (agent, fragment, records::WORK),
        };
        self.out.push(Effect::Owed { agent: agent.to_string(), fragment: fragment.to_string(), channel, turn: turn.to_string(), id: o.id, body: o.body });
    }

    fn draft(&mut self, agent: &str, fragment: &str, turn: &str, text: Option<String>) {
        self.out.push(Effect::Draft { agent: agent.to_string(), fragment: fragment.to_string(), turn: turn.to_string(), text });
    }

    // ---- agents ----

    fn set_agents(&mut self, agents: Vec<Agent>) {
        assert!(agents.len() <= limits::AGENTS_MAX, "the driver bounds the agents it reads");
        self.agents = agents;
        let gone: Vec<String> = self.state.turns.values().filter(|t| self.agent(&t.agent).is_none()).map(|t| t.id.clone()).collect();
        for id in gone {
            let t = self.state.turns.remove(&id).expect("listed");
            self.claiming.remove(&id);
            self.dirty = true;
            crate::ev!("turn.dropped", { "turn": id, "agent": t.agent, "why": "the agent left this computer" });
            if t.active() {
                self.tell_runtime(&id, Command::Forget { turn: id.clone() });
            }
        }
    }

    // ---- records ----

    fn record(&mut self, agent_fragment: &str, fragment: &str, record: Record, view: Option<ChatView>, since: i64) {
        let Some(agent) = self.agent(agent_fragment).cloned() else {
            crate::ev!("record.skipped", { "agent": agent_fragment, "fragment": fragment, "seq": record.seq, "why": "not an agent of this computer" });
            return;
        };
        let key = cursor_key(agent_fragment, fragment, &record.channel);
        let cursor = self.state.cursors.get(&key).copied().unwrap_or(0);
        if record.seq <= cursor {
            // A catch-up, a reconnect, or a socket's page read again: done.
            return;
        }
        // The cursor passes the record before anything it causes is done.
        self.state.cursors.insert(key, record.seq);
        self.dirty = true;
        if let Some(v) = view {
            self.views.insert(fragment.to_string(), v);
        }
        if record.at < since {
            // Said before the agent joined: not for it.
            return;
        }
        if record.principal.starts_with("anon:") {
            return;
        }
        match record.channel.as_str() {
            // its owner's words through one of its doors (docs/chat-records.md,
            // "A relayed message") are their message, as theirs, whichever
            // door and whoever carried them: the platform takes such a record
            // only through a door its owner opened, so no door need be linked
            // on this computer. Another agent of the chat carrying them is a
            // hand-off from that agent at most, never its owner.
            records::CHAT if self.relayed_for(&agent, fragment, &record) => {
                let via = records::relayed(&record.body).map(|r| r.via).unwrap_or_default();
                crate::ev!("relayed", { "agent": agent.fragment, "fragment": fragment, "seq": record.seq, "via": via });
                let as_owner = Record { principal: agent.owner.clone(), ..record };
                self.said(&agent, fragment, &as_owner);
            }
            // the agent's own words are not for it
            records::CHAT if record.principal != agent.identity => self.said(&agent, fragment, &record),
            records::TASKS if fragment == agent.fragment => {
                // Only the agent's own fragment (its cron, the platform's
                // `joined`: by the fragment's key, whose npub is the agent's
                // identity) and its owner ask it anything here. Another
                // agent, though it acts for the owner and may post here,
                // starts no routine: it would start a turn no hop counts.
                if record.principal != agent.owner && record.principal != agent.identity {
                    crate::ev!("task.ignored", { "agent": agent.fragment, "seq": record.seq, "principal": record.principal, "why": "only the agent's owner, or its own fragment, asks it on tasks" });
                    return;
                }
                self.task(&agent, &record)
            }
            _ => {}
        }
    }

    /// An owner's answer to a card of the agent's that no running turn of
    /// this life held when its follower read it (driver.rs `feed`), with
    /// the journal read for it, or why it could not be (final: never a
    /// failed read, which its follower reads again). The card's first answer starts a
    /// fresh turn, caused by the answer itself, told the card's turn as
    /// its note (cards.rs); a card that cannot be rebuilt is closed
    /// `history_unavailable` and starts nothing. Either way the cursor
    /// passes the answer, as it passes any record.
    fn card_record(&mut self, agent: &str, fragment: &str, record: Record, view: Option<ChatView>, since: i64, history: Result<crate::history::Journal, crate::history::Unavailable>) {
        let Some(a) = self.agent(agent).cloned() else { return };
        if record.seq <= self.state.cursors.get(&cursor_key(agent, fragment, &record.channel)).copied().unwrap_or(0) {
            return;
        }
        let Said::PromptResponse { prompt, card, .. } = records::said(&record.body) else { unreachable!("the driver sends a card's answer alone") };
        // a turn of this life holds it after all (it was read as the turn
        // asked): the card's answer, as any is
        let held = self.state.turns.values().any(|t| t.active() && t.agent == agent && t.fragment == fragment && card.as_ref().is_none_or(|c| c.agent == a.identity && c.turn == t.id) && t.prompts.iter().any(|p| p.id == prompt));
        if held || record.at < since {
            self.record(agent, fragment, record, view, since);
            return;
        }
        if let Some(v) = &view {
            self.views.insert(fragment.to_string(), v.clone());
        }
        let names = self.views.get(fragment).map(|v| v.names.clone()).unwrap_or_default();
        let name = |p: &str| names.get(p).cloned().unwrap_or_else(|| "someone".into());
        let resolved = history.and_then(|j| crate::cards::resolve(&a, &record, &j.work, &j.chat, j.cause.as_ref(), name, self.now));
        let (seq, at, asker) = (record.seq, record.at, record.principal.clone());
        self.record(agent, fragment, record, view, since);
        match resolved {
            Ok(None) => {}
            Ok(Some(r)) => {
                let turn = r.closed["turn"].as_str().expect("a closure names its turn").to_string();
                crate::ev!("card.late", { "agent": agent, "fragment": fragment, "seq": seq, "turn": turn, "outcome": r.closed["outcome"], "fresh": r.fresh.is_some() });
                self.post(agent, fragment, records::WORK, records::work_id(&turn, &format!("pc:{prompt}")), r.closed, Vec::new());
                let Some(fresh) = r.fresh else { return };
                // admitted as the answer's own turn, never Tell or Other for
                // a question another turn asks meanwhile
                let cause = Cause { fragment: fragment.to_string(), channel: records::CHAT.to_string(), seq };
                let id = records::turn_id(agent, fragment, records::CHAT, seq);
                let m = Message { text: fresh.text, to: vec![a.identity.clone()], attachments: Vec::new(), turn: None, hop: 0, reply_to: None, return_to: None };
                self.admit(&a, fragment, cause, at, &asker, name(&asker), &m, 0, Ask::Message);
                if let Some(t) = self.state.turns.get_mut(&id).filter(|t| t.phase == Phase::Queued) {
                    t.note = Some(fresh.note);
                }
            }
            Err(why) => {
                crate::ev!("card.unavailable", { "agent": agent, "fragment": fragment, "seq": seq, "why": why.why() });
                // closed on the card's own turn; an answer that names no
                // card has no turn to close
                if let Some(c) = card {
                    self.post(agent, fragment, records::WORK, records::work_id(&c.turn, &format!("pc:{prompt}")), records::turn_prompt_closed(&c.turn, &prompt, Closed::Unavailable, None), Vec::new());
                }
            }
        }
    }

    /// The prompts this life's running turns hold open, by agent and chat:
    /// their answers are theirs, read with no journal (driver.rs `feed`).
    pub fn held(&self) -> BTreeSet<(String, String, String)> {
        self.state.turns.values().filter(|t| t.active()).flat_map(|t| t.prompts.iter().filter(|p| !p.closed).map(|p| (t.agent.clone(), t.fragment.clone(), p.id.clone()))).collect()
    }

    /// Whether `record` relays `agent`'s owner through `agent`'s own door,
    /// carried by the agent itself or by a key that is none of the chat's
    /// members (a hosted connector publishes as its own fragment's key), as
    /// the chat's page reads it.
    fn relayed_for(&self, agent: &Agent, fragment: &str, record: &Record) -> bool {
        let member = |v: &ChatView| v.agents.contains(&record.principal) || v.people.contains(&record.principal);
        let carrier = record.principal == agent.identity || self.views.get(fragment).is_some_and(|v| !member(v));
        carrier && records::relayed(&record.body).is_some_and(|r| r.for_ == agent.owner && r.agent == agent.fragment)
    }

    fn said(&mut self, agent: &Agent, fragment: &str, record: &Record) {
        match records::said(&record.body) {
            Said::Message(m) => {
                let view = self.views.get(fragment).cloned().unwrap_or_default();
                let hop = self.addressed(agent, fragment, &view, &record.principal, &m);
                if hop == Some(0) && self.told(agent, fragment, &record.principal, &view, &m, record.seq) {
                    return;
                }
                if let Some(hop) = hop {
                    let asker_name = view.names.get(&record.principal).cloned().unwrap_or_else(|| "someone".into());
                    let cause = Cause { fragment: fragment.to_string(), channel: record.channel.clone(), seq: record.seq };
                    self.admit(agent, fragment, cause, record.at, &record.principal, asker_name, &m, hop, Ask::Message);
                }
            }
            Said::Stop { turn } => self.stop(agent, fragment, &record.principal, turn.as_deref()),
            Said::PromptResponse { prompt, option, text, card } => self.answer(agent, fragment, &record.principal, &prompt, &option, text, card.as_ref(), record.seq),
            Said::Command(c) => self.command(agent, fragment, record, c),
            Said::Other => {}
        }
    }

    /// A command of the runtime's menu (`How`), from the agent's owner alone,
    /// for the agents its `to` names, else the lead. Anyone else's, and one
    /// not on the menu, is passed over: nothing outside the menu reaches the
    /// runtime as a command.
    fn command(&mut self, agent: &Agent, fragment: &str, record: &Record, c: CommandSaid) {
        let view = self.views.get(fragment).cloned().unwrap_or_default();
        let for_it = if c.to.is_empty() { view.lead() == Some(agent.identity.as_str()) } else { c.to.iter().any(|t| t == &agent.identity) };
        if !for_it {
            return;
        }
        if record.principal != agent.owner {
            crate::ev!("command.ignored", { "agent": agent.fragment, "fragment": fragment, "seq": record.seq, "command": c.name, "why": "only the agent's owner sends it commands" });
            return;
        }
        let Some(item) = self.menu_of(&agent.fragment).iter().find(|m| m.name == c.name).copied() else {
            crate::ev!("command.ignored", { "agent": agent.fragment, "fragment": fragment, "seq": record.seq, "command": c.name, "why": "not one of its runtime's commands" });
            return;
        };
        let asker_name = view.names.get(&record.principal).cloned().unwrap_or_else(|| "someone".into());
        let cause = Cause { fragment: fragment.to_string(), channel: record.channel.clone(), seq: record.seq };
        let text = if c.args.is_empty() { format!("/{}", c.name) } else { format!("/{} {}", c.name, c.args) };
        let as_command = Message { text: text.clone(), ..Message::default() };
        let as_message = Message { text: c.args.clone(), ..Message::default() };
        let refused = item.refuse.iter().find(|(word, _)| c.args.split_whitespace().any(|w| w.eq_ignore_ascii_case(word))).map(|(_, why)| *why);
        crate::ev!("command", { "agent": agent.fragment, "fragment": fragment, "seq": record.seq, "command": c.name, "refused": refused.is_some() });
        let has_args = !c.args.is_empty();
        let running = self.state.turns.values().find(|t| t.agent == agent.fragment && t.fragment == fragment && t.active()).map(|t| t.id.clone());
        match item.how {
            _ if refused.is_some() => self.admit(agent, fragment, cause, record.at, &record.principal, asker_name, &as_command, 0, Ask::Refused(refused.unwrap_or_default())),
            How::Stop => self.stop_all(&agent.fragment, fragment),
            How::Restart => {
                self.stop_all(&agent.fragment, fragment);
                self.admit(agent, fragment, cause, record.at, &record.principal, asker_name, &as_command, 0, Ask::Command);
            }
            How::Message if has_args => self.admit(agent, fragment, cause, record.at, &record.principal, asker_name, &as_message, 0, Ask::Message),
            How::Steer if has_args && running.is_none() => self.admit(agent, fragment, cause, record.at, &record.principal, asker_name, &as_message, 0, Ask::Message),
            How::Steer | How::Aside if has_args => {
                let id = records::turn_id(&agent.fragment, &cause.fragment, &cause.channel, cause.seq);
                let turn = running.filter(|_| item.how == How::Steer);
                crate::ev!("command.aside", { "agent": agent.fragment, "fragment": fragment, "seq": record.seq, "turn": turn });
                let simplex = self.mirror(&agent.fragment) == Some(fragment);
                // its agent's runtime, whose menu it is on
                let runtime = self.runtime_of(&agent.fragment);
                self.out.push(Effect::Runtime { runtime, command: Command::Aside { agent: agent.fragment.clone(), fragment: fragment.to_string(), chat_name: label(fragment).to_string(), simplex, turn, id, by: record.principal.clone(), by_name: asker_name, text } });
            }
            _ => self.admit(agent, fragment, cause, record.at, &record.principal, asker_name, &as_command, 0, Ask::Command),
        }
    }

    /// The agent's owner stopped it in this chat (`How::Stop`): its turn
    /// running there is stopped, whoever asked it, and its waiting ones end
    /// stopped, never run.
    fn stop_all(&mut self, agent: &str, fragment: &str) {
        let mut mine: Vec<(u64, String, Phase)> = self.state.turns.values().filter(|t| t.agent == agent && t.fragment == fragment && t.phase != Phase::Ended).map(|t| (t.order, t.id.clone(), t.phase)).collect();
        mine.sort_by_key(|(order, _, _)| *order);
        for (_, id, phase) in mine {
            crate::ev!("turn.stopped", { "turn": id, "phase": format!("{phase:?}").to_lowercase(), "by": "command" });
            match phase {
                Phase::Queued => self.end_unrun(&id, Outcome::Stopped, false),
                _ => self.stop_running(agent, fragment, &id),
            }
        }
    }

    fn task(&mut self, agent: &Agent, record: &Record) {
        match records::task(&record.body) {
            Task::Routine { text, chat } => {
                let cause = Cause { fragment: agent.fragment.clone(), channel: record.channel.clone(), seq: record.seq };
                let m = Message { text, ..Message::default() };
                let owner = agent.owner.clone();
                self.admit(agent, &chat, cause, record.at, &owner, "your routine".into(), &m, 0, Ask::Routine);
            }
            Task::Joined { fragment } => self.out.push(Effect::Discover { agent: agent.fragment.clone(), joined: fragment }),
            // Only its own bridge answers it (as itself, `owe_answer`): a
            // turn asked as the agent that answered (its identity and name,
            // which the runtime is told it is from: Paul, 2026-10-30), its
            // owner's business in that chat still (`who_answers`: their
            // Stop, their words to a question it asks), and no fewer hops
            // deep than the turn that answered, so a callback resets no
            // depth and spends that chat's budget.
            Task::Answer { chat, text, by, name, hop, refused } if record.principal == agent.identity => {
                let cause = Cause { fragment: agent.fragment.clone(), channel: record.channel.clone(), seq: record.seq };
                let m = Message { text: text.clone(), ..Message::default() };
                // a refused question's answer is a turn that never runs, its
                // end saying why: no runtime hears it, so nothing asks again
                let ask = if refused { Ask::Refused(&text) } else { Ask::Message };
                let hop = if refused { 0 } else { hop.clamp(1, limits::HOPS_MAX) };
                self.admit(agent, &chat, cause, record.at, &by, name, &m, hop, ask);
            }
            Task::Answer { .. } => crate::ev!("task.ignored", { "agent": agent.fragment, "seq": record.seq, "why": "only the agent's own bridge posts its answers" }),
            Task::Other => {}
        }
    }

    /// Whether a message is for `agent`, and the hop its turn is at.
    fn addressed(&self, agent: &Agent, fragment: &str, view: &ChatView, principal: &str, m: &Message) -> Option<u32> {
        let from_agent = view.agents.iter().any(|a| a == principal) || self.agents.iter().any(|a| a.identity == principal);
        if from_agent {
            if !m.to.iter().any(|t| t == &agent.identity) {
                return None;
            }
            let hop = self.hop_of(fragment, principal, m.turn.as_deref(), m.hop);
            // an ask past the cap is still a turn, refused (`admit`), so its
            // answer says why; any other hand-off past it is let go
            let asks = m.return_to.is_some() && self.agents.iter().any(|a| a.identity == principal);
            if hop > limits::HOPS_MAX && !asks {
                crate::ev!("handoff.too_deep", { "agent": agent.fragment, "fragment": fragment, "from": principal, "hop": hop, "claimed": m.hop });
                return None;
            }
            return Some(hop);
        }
        if !m.to.is_empty() {
            return m.to.iter().any(|t| t == &agent.identity).then_some(0);
        }
        let words = records::mentions(&m.text);
        let named: Vec<&Agent> = self.agents.iter().filter(|a| view.agents.contains(&a.identity) && words.iter().any(|w| answers_to(a, w))).collect();
        if !named.is_empty() {
            return named.iter().any(|a| a.identity == agent.identity).then_some(0);
        }
        (view.lead() == Some(agent.identity.as_str())).then_some(0)
    }

    /// How many hand-offs led to a record an agent posted in `fragment`:
    /// this bridge's count, never fewer than the record `claimed`. For an
    /// agent of this computer it is one past the turn the agent is in: its
    /// turn running in this chat, or the turn here the record names (`turn`)
    /// when that ended within `ENDED_HOPS_MS` (a reply read after its turn
    /// was let go); and its deepest turn running elsewhere (a post from a
    /// turn into another chat, as `fragment ask` makes), the deeper of the
    /// two; in no turn at all, the last hop allowed: answered once, handing
    /// on nothing. So a post made around the bridge (the CLI, the API: no
    /// `hop`, or `hop: 0`, and no `turn`) counts as the reply would. Another
    /// computer's agent, whose turns this bridge cannot see, is one hop at
    /// least, as it claims; the chat's budget holds it too.
    fn hop_of(&self, fragment: &str, principal: &str, named: Option<&str>, claimed: u32) -> u32 {
        let Some(poster) = self.agents.iter().find(|a| a.identity == principal) else {
            return claimed.max(1);
        };
        let running = |t: &&Turn| t.agent == poster.fragment && t.active();
        let here = self.state.turns.values().filter(running).find(|t| t.fragment == fragment).map(|t| t.hop).or_else(|| {
            let fresh = |e: &&EndedTurn| self.now.saturating_sub(e.at) <= limits::ENDED_HOPS_MS;
            let named = named?;
            self.ended.iter().rev().filter(fresh).find(|e| e.id == named && e.agent == poster.fragment && e.fragment == fragment).map(|e| e.hop)
        });
        let elsewhere = self.state.turns.values().filter(running).filter(|t| t.fragment != fragment).map(|t| t.hop).max();
        let from = here.into_iter().chain(elsewhere).max().unwrap_or(limits::HOPS_MAX - 1);
        from.saturating_add(1).max(claimed)
    }

    /// Whether the chat's agents may start another turn of each other at
    /// `at` (a causing record's time): fewer than
    /// `AGENT_TURNS_PER_CHAT_MAX` in the window before it. Times older than
    /// the window are let go; a later one (another follower's, ahead of
    /// this one) still counts.
    fn agent_turn_allowed(&mut self, chat: &str, at: i64) -> bool {
        let Some(times) = self.state.agent_turns.get_mut(chat) else { return true };
        let before = times.len();
        times.retain(|t| *t > at.saturating_sub(limits::AGENT_TURNS_WINDOW_MS));
        if times.len() != before {
            self.dirty = true;
        }
        if times.is_empty() {
            self.state.agent_turns.remove(chat);
            return true;
        }
        times.len() < limits::AGENT_TURNS_PER_CHAT_MAX
    }

    /// Counts a turn the chat's agents started of each other at `at`.
    fn spend_agent_turn(&mut self, chat: &str, at: i64) {
        let times = self.state.agent_turns.entry(chat.to_string()).or_default();
        let place = times.partition_point(|t| *t <= at);
        times.insert(place, at);
        // the newest only: the oldest past the cap could never refuse one
        if times.len() > limits::AGENT_TURNS_PER_CHAT_MAX {
            let over = times.len() - limits::AGENT_TURNS_PER_CHAT_MAX;
            times.drain(..over);
        }
        self.dirty = true;
        // bounded: the chat counted least lately goes
        while self.state.agent_turns.len() > limits::AGENT_TURN_CHATS_MAX {
            let stalest = self.state.agent_turns.iter().min_by_key(|(_, t)| t.last().copied().unwrap_or(i64::MIN)).map(|(c, _)| c.clone()).expect("over the cap, so not empty");
            self.state.agent_turns.remove(&stalest);
        }
    }

    /// A turn for `agent` in `fragment`, caused by a record at `at` (the
    /// platform's time); `hop` past zero is a hand-off from another agent,
    /// which the chat's budget counts.
    #[allow(clippy::too_many_arguments)]
    fn admit(&mut self, agent: &Agent, fragment: &str, cause: Cause, at: i64, asker: &str, asker_name: String, m: &Message, hop: u32, ask: Ask) {
        let id = records::turn_id(&agent.fragment, &cause.fragment, &cause.channel, cause.seq);
        if self.state.turns.contains_key(&id) {
            // The cursor makes this impossible; a state that says otherwise
            // is not trusted to go on.
            panic!("turn {id} admitted twice: the cursor of {} is behind its turns", cursor_key(&agent.fragment, &cause.fragment, &cause.channel));
        }
        let routine = ask == Ask::Routine;
        assert!(ask == Ask::Message || hop == 0, "a routine or a command is its owner's ask, no hand-off");
        let waiting = self.state.turns.values().filter(|t| t.agent == agent.fragment && t.fragment == fragment && t.phase == Phase::Queued).count();
        let refusal = if let Ask::Refused(why) = ask {
            Some(why.to_string())
        } else if hop > limits::HOPS_MAX {
            Some(refused_deep())
        } else if hop > 0 && !self.agent_turn_allowed(fragment, at) {
            Some(refused_budget())
        } else if waiting >= limits::QUEUED_PER_CHAT_MAX {
            Some(REFUSED_QUEUED.to_string())
        } else if self.state.turns.len() >= limits::TURNS_OPEN_MAX {
            Some(REFUSED_BUSY.to_string())
        } else {
            None
        };
        // an ask's answer goes back only to another agent of this computer,
        // whose own `tasks` its bridge posts it on; a refused one's too,
        // which says why and wakes no one (`owe_answer`)
        let ret = m.return_to.as_ref().and_then(|chat| {
            let asking = self.agents.iter().find(|a| a.identity == asker && a.fragment != agent.fragment)?;
            // as its chat shows it, else its label while that is unread
            let name = self.views.get(fragment).and_then(|v| v.names.get(&agent.identity)).cloned().unwrap_or_else(|| agent.name.clone());
            Some(Return { agent: asking.fragment.clone(), chat: chat.clone(), name, said: String::new(), files: 0 })
        });
        self.state.admitted += 1;
        let turn = Turn {
            id: id.clone(),
            agent: agent.fragment.clone(),
            fragment: fragment.to_string(),
            cause,
            asker: asker.to_string(),
            asker_name,
            text: records::cut_bytes(&m.text, limits::MESSAGE_TEXT_MAX_BYTES),
            attachments: m.attachments.clone(),
            routine,
            hop,
            order: self.state.admitted,
            phase: Phase::Queued,
            last_ms: self.now,
            steps: 0,
            replies: 0,
            interim: 0,
            calls: 0,
            last_part: 0,
            prompts: Vec::new(),
            stop_requested: false,
            owed: Vec::new(),
            asking: false,
            command: matches!(ask, Ask::Command | Ask::Refused(_)),
            reply_to: m.reply_to,
            note: None,
            ret,
            continues: None,
            continue_after: false,
            open: None,
        };
        self.dirty = true;
        if let Some(why) = refusal {
            crate::ev!("turn.refused", { "turn": id, "agent": agent.fragment, "fragment": fragment, "why": why });
            let outcome = Outcome::Error(why);
            if self.state.turns.len() < limits::TURNS_OPEN_MAX {
                self.state.turns.insert(id.clone(), turn);
                // refused at admission: its answer runs nothing
                self.end_unrun(&id, outcome, true);
            } else {
                // Past the bound of turns held, its two records are posted
                // and not kept: a crash before they land loses them.
                crate::ev!("owed.unkept", { "turn": id, "why": "the computer holds as many turns as it may" });
                let start = records::turn_start(&id, asker, &agent.identity, &turn.cause, &self.life);
                self.post(&agent.fragment, fragment, records::WORK, records::work_id(&id, "start"), start, Vec::new());
                self.post(&agent.fragment, fragment, records::WORK, records::work_id(&id, "end"), records::turn_end(&id, &outcome), Vec::new());
            }
            return;
        }
        crate::ev!("turn.admitted", { "turn": id, "agent": agent.fragment, "fragment": fragment, "seq": turn.cause.seq, "hop": hop });
        if hop > 0 {
            self.spend_agent_turn(fragment, at);
        }
        self.state.turns.insert(id, turn);
        self.pump(&agent.fragment, fragment);
    }

    /// A held turn ends without running here (refused, or stopped while it
    /// waited): it owes its claim, then its end, so it has both records as
    /// every turn does, and it is kept until both are answered. When
    /// another life claimed it, the first is a 409, and so is the second if
    /// that life ended it.
    fn end_unrun(&mut self, id: &str, outcome: Outcome, refused: bool) {
        self.claiming.remove(id);
        let agent = self.state.turns[id].agent.clone();
        let identity = self.agent(&agent).expect("a held turn's agent is on this computer").identity.clone();
        let life = self.life.clone();
        let t = self.state.turns.get_mut(id).expect("held");
        assert_eq!(t.phase, Phase::Queued, "a turn ends unrun only from the queue");
        let start = Owed { id: records::work_id(id, "start"), body: records::turn_start(id, &t.asker, &identity, &t.cause, &life), to: None };
        let end = Owed { id: records::work_id(id, "end"), body: records::turn_end(id, &outcome), to: None };
        t.phase = Phase::Ended;
        t.owed = vec![start.clone(), end.clone()];
        let fragment = t.fragment.clone();
        self.dirty = true;
        self.owe(&agent, &fragment, id, start);
        self.owe(&agent, &fragment, id, end);
        self.owe_answer(id, &outcome, refused);
    }

    /// An ended turn that answers an ask owes its answer too: posted as its
    /// asking agent on that agent's own `tasks` (`records::answer`, under
    /// `answer_id`, so any life's is the one record), which starts that
    /// agent's turn in the chat it asked from. Its outcome is its end's: a
    /// cut turn is answered `lost`, never run again for a better answer. A
    /// question refused at admission (past the hops or the chat's budget,
    /// its queue full) is answered `refused`: a turn there that never runs,
    /// so its asker's owner sees why, and no answer asks again.
    fn owe_answer(&mut self, id: &str, outcome: &Outcome, refused: bool) {
        let t = &self.state.turns[id];
        let Some(r) = t.ret.clone() else { return };
        assert_eq!(t.phase, Phase::Ended, "only an ended turn answers");
        if self.agent(&r.agent).is_none() {
            crate::ev!("answer.dropped", { "turn": id, "why": "its asking agent left this computer" });
            return;
        }
        let by = self.agent(&t.agent).expect("a held turn's agent is on this computer");
        let text = match outcome {
            Outcome::Error(why) if refused => records::refused_text(&r.name, &t.fragment, &t.text, why),
            _ => records::answer_text(&r.name, &t.fragment, &t.text, &r.said, r.files, outcome),
        };
        let hop = t.hop.saturating_add(1).min(limits::HOPS_MAX);
        let body = records::answer(&r.chat, &text, &by.identity, &r.name, &t.cause, id, outcome, hop, refused);
        let o = Owed { id: records::answer_id(id), body, to: Some(r.agent.clone()) };
        let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
        self.state.turns.get_mut(id).expect("held").owed.push(o.clone());
        self.dirty = true;
        crate::ev!("answer.owed", { "turn": id, "to": r.agent, "chat": r.chat });
        self.owe(&agent, &fragment, id, o);
    }

    /// A turn that may not run here (refused its claim, or its agent gone
    /// from its chat) is let go, unless it answers an ask: then it ends,
    /// owing that answer (`why`), so the asking agent hears it never ran.
    fn let_go(&mut self, id: &str, why: &str) {
        self.claiming.remove(id);
        let t = self.state.turns.get_mut(id).expect("held");
        self.dirty = true;
        if t.phase == Phase::Ended {
            // what it owes its chat goes; its answer goes elsewhere
            t.owed.retain(|o| o.to.is_some());
        }
        if t.phase == Phase::Ended || t.ret.is_none() {
            if t.owed.is_empty() {
                self.state.turns.remove(id);
            }
            return;
        }
        t.phase = Phase::Ended;
        t.owed.clear();
        t.prompts.clear();
        t.asking = false;
        self.owe_answer(id, &Outcome::Error(why.to_string()), false);
        if self.state.turns[id].owed.is_empty() {
            self.state.turns.remove(id);
        }
    }

    /// The lane is done with one of a turn's owed records; a turn that owes
    /// none is let go. Done means answered (appended, a replay, or refused:
    /// a 409 is another life's end of the turn, a 403/404 a chat it may no
    /// longer post in) or given up after the lane's tries
    /// (`limits::POST_TRIES_MAX`, with jitter, on a transport error, 429 or
    /// 5xx), so a record that keeps failing holds its turn for one life's
    /// tries at most, then goes.
    fn posted(&mut self, id: &str, post: &str, kept: bool) {
        let Some(t) = self.state.turns.get_mut(id) else {
            crate::ev!("owed.stale", { "turn": id, "id": post, "why": "no turn held by that id" });
            return;
        };
        if t.phase != Phase::Ended || !t.owed.iter().any(|o| o.id == post) {
            crate::ev!("owed.stale", { "turn": id, "id": post, "why": "the turn does not owe it" });
            return;
        }
        // its lost end is this life's on the journal (not another life's,
        // which ran or ended it after this `/data` was saved): continued once
        let cut = (kept && t.continue_after && post == records::work_id(id, "end")).then(|| {
            t.continue_after = false;
            t.clone()
        });
        t.owed.retain(|o| o.id != post);
        self.dirty = true;
        if t.owed.is_empty() {
            self.state.turns.remove(id);
            crate::ev!("turn.closed", { "turn": id });
        }
        if let Some(cut) = cut {
            self.continue_turn(cut);
        }
    }

    /// One automatic continuation of a turn a restart cut (durability phase
    /// 4: nobody types "continue"), queued in its place: the same agent,
    /// chat, cause, asker and depth, said by the computer, never as its
    /// asker's words (`CONTINUE_ASKER`, `CONTINUE_TEXT`), and told what was
    /// cut (its note: driver.rs, `cut_note`, from the cut turn by its id).
    /// Its id is the cut turn's continuation's (`records::continuation_id`),
    /// so its claim makes it one run whatever lives admit it; it never reruns
    /// what the cut turn did, and one that is cut in its turn is never
    /// continued (Pi's restart limit, ending visibly). Past the bounds of a
    /// chat's queue or the computer's turns it is not admitted.
    fn continue_turn(&mut self, cut: Turn) {
        let id = records::continuation_id(&cut.id);
        let waiting = self.state.turns.values().filter(|t| t.agent == cut.agent && t.fragment == cut.fragment && t.phase == Phase::Queued).count();
        if self.state.turns.contains_key(&id) || self.agent(&cut.agent).is_none() {
            return;
        }
        if waiting >= limits::QUEUED_PER_CHAT_MAX || self.state.turns.len() >= limits::TURNS_OPEN_MAX {
            crate::ev!("turn.uncontinued", { "turn": cut.id, "why": "its chat's queue or this computer's turns are full" });
            return;
        }
        crate::ev!("turn.continued", { "turn": id, "cut": cut.id, "agent": cut.agent, "fragment": cut.fragment });
        let turn = Turn {
            id: id.clone(),
            agent: cut.agent.clone(),
            fragment: cut.fragment.clone(),
            cause: cut.cause,
            asker: cut.asker,
            asker_name: CONTINUE_ASKER.into(),
            text: CONTINUE_TEXT.into(),
            attachments: Vec::new(),
            routine: cut.routine,
            hop: cut.hop,
            // its place: before what was said after the cut turn
            order: cut.order,
            phase: Phase::Queued,
            last_ms: self.now,
            // its own bounds, as any turn's: they bound the records under a
            // turn's id (the rest counted, not posted), not its work, and a
            // continuation is never continued
            steps: 0,
            replies: 0,
            interim: 0,
            calls: 0,
            last_part: 0,
            prompts: Vec::new(),
            stop_requested: false,
            owed: Vec::new(),
            asking: false,
            command: false,
            reply_to: None,
            note: None,
            ret: None,
            continues: Some(cut.id),
            continue_after: false,
            open: None,
        };
        self.state.turns.insert(id, turn);
        self.dirty = true;
        self.pump(&cut.agent, &cut.fragment);
    }

    /// Claims the chat's next turn when its runtime can take one and none
    /// of its agent's runs, or is being claimed, there. Nothing is persisted:
    /// the turn stays queued in the state until its claim is answered, so a
    /// life that ends meanwhile leaves it for the next to claim (a 409 then,
    /// if this claim had landed).
    fn pump(&mut self, agent: &str, fragment: &str) {
        let runtime = self.runtime_of(agent);
        match self.runtimes.get(&runtime) {
            Some(r) if r.connected => {}
            Some(_) => return,
            None => return self.refuse_queued(agent, fragment, &runtime),
        }
        let busy = self.state.turns.values().any(|t| t.agent == agent && t.fragment == fragment && (t.active() || self.claiming.contains(&t.id)));
        if busy {
            return;
        }
        let next = self.state.turns.values().filter(|t| t.agent == agent && t.fragment == fragment && t.phase == Phase::Queued).min_by_key(|t| t.order).map(|t| t.id.clone());
        let Some(id) = next else { return };
        let identity = self.agent(agent).expect("a held turn's agent is on this computer").identity.clone();
        let t = &self.state.turns[&id];
        let body = records::turn_start(&id, &t.asker, &identity, &t.cause, &self.life);
        let fragment = t.fragment.clone();
        self.claiming.insert(id.clone());
        crate::ev!("turn.claiming", { "turn": id, "agent": agent, "fragment": fragment });
        self.out.push(Effect::Claim { agent: agent.to_string(), fragment, turn: id.clone(), id: records::work_id(&id, "start"), body });
    }

    /// The agent's turns waiting in the chat end unrun, both their records
    /// posted, the end saying why (and an ask's answer too): its
    /// `agent.json` names a runtime this bridge does not run, so none of them
    /// could run here, and none is handed to another runtime. One being
    /// claimed is `hand`'s to end.
    fn refuse_queued(&mut self, agent: &str, fragment: &str, runtime: &str) {
        let queued: Vec<String> = self.state.turns.values().filter(|t| t.agent == agent && t.fragment == fragment && t.phase == Phase::Queued && !self.claiming.contains(&t.id)).map(|t| t.id.clone()).collect();
        for id in queued {
            crate::ev!("turn.refused", { "turn": id, "agent": agent, "fragment": fragment, "why": "its runtime is none this bridge runs", "runtime": runtime });
            self.end_unrun(&id, Outcome::Error(refused_runtime(runtime)), true);
        }
    }

    /// Every chat's queue moves (a claim may be due in any).
    fn pump_all(&mut self) {
        let chats: BTreeSet<(String, String)> = self.state.turns.values().map(|t| (t.agent.clone(), t.fragment.clone())).collect();
        for (agent, fragment) in chats {
            self.pump(&agent, &fragment);
        }
    }

    /// A claim's answer: this life runs the turn; or it is another life's,
    /// and ends as lost; or nothing is known, and it waits for the next
    /// tick's claim (never sooner, so a held computer is asked once a tick).
    fn claimed(&mut self, id: &str, answer: ClaimAnswer) {
        if !self.claiming.remove(id) {
            // Stopped, or its agent gone, while its claim was in flight.
            crate::ev!("claim.stale", { "turn": id, "answer": format!("{answer:?}") });
            return;
        }
        assert!(self.state.turns.get(id).is_some_and(|t| t.phase == Phase::Queued), "a turn being claimed is held, queued: {id}");
        match answer {
            ClaimAnswer::Ours { seq } => self.hand(id, seq),
            ClaimAnswer::Theirs => {
                // It owes its end as lost (a 409 itself when that life ended
                // it, and let go then like any answered end).
                let t = self.state.turns.get_mut(id).expect("checked");
                let end = Owed { id: records::work_id(id, "end"), body: records::turn_end(id, &Outcome::Error(LOST.into())), to: None };
                t.phase = Phase::Ended;
                t.owed = vec![end.clone()];
                // that life left it with no end of its own when this one's
                // lands as this life's (`posted`): it was cut, and is
                // continued once; one that life ended refuses this end (409)
                t.continue_after = continued(t);
                let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
                self.dirty = true;
                crate::ev!("turn.lost", { "turn": id, "agent": agent, "why": "another life claimed it" });
                // that life ran it after this /data was saved: the runtime
                // remembers nothing of it, and its next turn here is told
                let (newest, more) = self.forgotten.entry((agent.clone(), fragment.clone())).or_default();
                newest.push_back(id.to_string());
                if newest.len() > limits::NOTE_FORGOTTEN_MAX {
                    newest.pop_front();
                    *more = more.saturating_add(1);
                }
                assert!(newest.len() <= limits::NOTE_FORGOTTEN_MAX, "a note names its newest forgotten turns");
                self.owe(&agent, &fragment, id, end);
                self.owe_answer(id, &Outcome::Error(LOST.into()), false);
                self.pump(&agent, &fragment);
            }
            ClaimAnswer::Refused => {
                let t = &self.state.turns[id];
                let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
                crate::ev!("turn.dropped", { "turn": id, "agent": agent, "why": "the agent may not post on the chat's work" });
                self.let_go(id, &format!("it may not answer in {fragment}"));
                self.pump(&agent, &fragment);
            }
            ClaimAnswer::Unanswered => crate::ev!("claim.unanswered", { "turn": id }),
        }
    }

    /// Hands a turn this life claimed to the runtime: from here a restart
    /// ends it. Its note (what a restart cut of the agent's turn before it
    /// in this chat) is the driver's to read from the journal before the
    /// runtime hears of the turn, as its attachments are.
    fn hand(&mut self, id: &str, claim_seq: Option<u64>) {
        let agent = self.state.turns[id].agent.clone();
        let a = self.agent(&agent).cloned().expect("a held turn's agent is on this computer");
        let now = self.now;
        let fragment = self.state.turns[id].fragment.clone();
        let runtime = self.runtime_of(&agent);
        let Some(menu) = self.runtimes.get(&runtime).map(|r| r.menu) else {
            // its agent's runtime changed, while its claim was in flight, to
            // one this bridge does not run: claimed, it ends saying so
            let t = self.state.turns.get_mut(id).expect("checked");
            t.phase = Phase::Running;
            t.last_ms = now;
            crate::ev!("turn.refused", { "turn": id, "agent": agent, "fragment": fragment, "why": "its runtime is none this bridge runs", "runtime": runtime });
            return self.end(id, Outcome::Error(refused_runtime(&runtime)), Closed::Expired);
        };
        // what another life ran here that the runtime does not remember,
        // told once: this turn takes it
        let (forgotten, forgotten_more) = self.forgotten.remove(&(agent.clone(), fragment.clone())).map(|(newest, more)| (Vec::from(newest), more)).unwrap_or_default();
        // the menu, once a life in each chat, as the agent first runs there
        if !menu.is_empty() && self.published.insert((agent.clone(), fragment.clone())) {
            self.post(&agent, &fragment, records::WORK, records::menu_id(&self.life), records::commands(&a.identity, menu), Vec::new());
        }
        // a cut turn's continuation is shown as it starts
        let continues = self.state.turns[id].continues.clone();
        if continues.is_some() {
            self.post(&agent, &fragment, records::WORK, records::work_id(id, "continued"), records::turn_notice(id, NoticeCategory::Info, CONTINUE_NOTICE), Vec::new());
        }
        // the runtime's notices in this chat are this turn's from here
        self.noticed.retain(|n| !(n.agent == agent && n.fragment == fragment));
        self.noticed.push_back(Noticed { agent: agent.clone(), fragment, turn: id.to_string(), notices: 0 });
        while self.noticed.len() > limits::NOTICE_CHATS_MAX {
            self.noticed.pop_front();
        }
        // its chat its owner's phone (its SimpleX chat): the runtime is told
        let simplex = self.mirror(&agent) == Some(self.state.turns[id].fragment.as_str());
        let t = self.state.turns.get_mut(id).expect("checked");
        t.phase = Phase::Running;
        t.last_ms = now;
        self.dirty = true;
        let start = TurnStart {
            turn: id.to_string(),
            agent: a,
            fragment: t.fragment.clone(),
            chat_name: label(&t.fragment).to_string(),
            seq: t.cause.seq,
            asker: t.asker.clone(),
            asker_name: t.asker_name.clone(),
            text: t.text.clone(),
            attachments: t.attachments.clone(),
            files: Vec::new(),
            routine: t.routine,
            claim_seq,
            note: t.note.clone(),
            forgotten,
            forgotten_more,
            command: t.command,
            reply_to: t.reply_to,
            quote: None,
            simplex,
            continues,
        };
        crate::ev!("turn.handed", { "turn": id, "agent": agent, "fragment": t.fragment, "command": t.command, "runtime": runtime });
        self.handed.insert(id.to_string(), runtime.clone());
        self.out.push(Effect::Runtime { runtime, command: Command::Start(Box::new(start)) });
    }

    // ---- Stop and answers ----

    /// A message sent while the agent's turn in this chat waits on a
    /// question: handed to that turn as its answer (true), never queued
    /// behind it, where the turn would wait for it until the question's
    /// life ran out. Its words, its files (photos sent as the answer), or
    /// both: the files go with it (`Attached`). A question in words
    /// (`asking`) takes its asker's message as its answer
    /// (`Command::Tell`); a card that takes words (a clarify's "Other"), its
    /// owner's, as that option answered in those words
    /// (`answered`), as if typed on the card. A card that takes no words
    /// (an approval's Allow and Deny) takes a press alone: a message
    /// meanwhile is queued, as anyone else's is.
    /// Only a turn this life runs asks (its claim was answered as this
    /// life's; a restart ends every turn an earlier life ran), so an answer
    /// goes only to the life that claimed its turn. Read by a later life, it
    /// asks nothing, and the message is a turn of its own, claimed and run
    /// as any other.
    fn told(&mut self, agent: &Agent, fragment: &str, principal: &str, view: &ChatView, m: &Message, seq: u64) -> bool {
        if m.text.trim().is_empty() && m.attachments.is_empty() {
            return false;
        }
        let text = records::cut_bytes(&m.text, limits::MESSAGE_TEXT_MAX_BYTES);
        let by_name = view.names.get(principal).cloned().unwrap_or_else(|| "someone".into());
        let attached = (!m.attachments.is_empty()).then(|| Attached { agent: agent.fragment.clone(), fragment: fragment.to_string(), by_name: by_name.clone(), refs: m.attachments.clone() });
        let mine = |t: &&Turn| t.agent == agent.fragment && t.fragment == fragment;
        if let Some((id, answers)) = self.state.turns.values().filter(mine).find(|t| t.asking && t.active()).map(|t| (t.id.clone(), self.who_answers(t))) {
            if answers != principal {
                return false;
            }
            let t = self.state.turns.get_mut(&id).expect("found");
            t.asking = false;
            t.last_ms = self.now;
            self.dirty = true;
            crate::ev!("turn.told", { "turn": id, "seq": seq, "files": m.attachments.len() });
            self.tell_runtime(&id, Command::Tell { turn: id.clone(), seq, by: principal.to_string(), by_name, text, attached });
            return true;
        }
        // its card that takes words, the oldest open one: its owner's alone
        // to answer, as its options are
        let carded = self.state.turns.values().filter(mine).filter(|t| t.phase == Phase::Waiting).find_map(|t| t.prompts.iter().find(|p| !p.closed && !p.words.is_empty()).map(|p| (t.id.clone(), p.id.clone(), p.words[0].clone())));
        let Some((id, prompt, option)) = carded else { return false };
        if principal != agent.owner {
            return false;
        }
        // what the card shows it was answered with: the words, or the files' names
        let shown = if text.trim().is_empty() { m.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ") } else { text.trim().to_string() };
        crate::ev!("prompt.told", { "turn": id, "prompt": prompt, "seq": seq, "files": m.attachments.len() });
        self.answered(&agent.fragment, fragment, &id, &prompt, &option, principal, Some(shown), Some(text), attached, seq);
        true
    }

    /// Whom a turn's question in words waits for, and who stops it: its
    /// asker, but a turn caused on its agent's own `tasks` (a routine, an
    /// answer) is its owner's business in its chat, whoever is named as
    /// asking it (an answer names the agent that answered).
    fn who_answers(&self, t: &Turn) -> String {
        match self.agent(&t.agent) {
            Some(a) if t.cause.channel == records::TASKS => a.owner.clone(),
            _ => t.asker.clone(),
        }
    }

    fn stop(&mut self, agent: &Agent, fragment: &str, principal: &str, named: Option<&str>) {
        let target = self
            .state
            .turns
            .values()
            .filter(|t| t.agent == agent.fragment && t.fragment == fragment)
            .filter(|t| match named {
                // a cut turn's Stop is its continuation's too (a page that
                // showed the cut turn running names it)
                Some(id) => t.id == id || t.continues.as_deref() == Some(id),
                None => t.active(),
            })
            .min_by_key(|t| t.phase == Phase::Ended)
            .map(|t| (t.id.clone(), self.who_answers(t), t.phase));
        let Some((id, answers, phase)) = target else {
            crate::ev!("stop.ignored", { "agent": agent.fragment, "fragment": fragment, "why": "no such turn" });
            return;
        };
        if answers != principal {
            crate::ev!("stop.ignored", { "turn": id, "why": "only the turn's asker (its owner, for a routine's or an answer's) stops it" });
            return;
        }
        match phase {
            Phase::Queued => {
                // Never run here (a claim in flight is answered to no one).
                crate::ev!("turn.stopped", { "turn": id, "phase": "queued" });
                self.end_unrun(&id, Outcome::Stopped, false);
                self.pump(&agent.fragment, fragment);
            }
            Phase::Ended => {
                // a restart cut it, and its lost end is not yet this life's
                // (`posted`): stopped, it is never continued
                let t = self.state.turns.get_mut(&id).expect("found");
                if t.continue_after {
                    t.continue_after = false;
                    self.dirty = true;
                    crate::ev!("turn.uncontinued", { "turn": id, "why": "its asker stopped it" });
                } else {
                    crate::ev!("stop.ignored", { "turn": id, "why": "it is over" });
                }
            }
            Phase::Running | Phase::Waiting => self.stop_running(&agent.fragment, fragment, &id),
        }
    }

    /// Stops a turn this life runs: its open cards close as stopped, and the
    /// runtime is told (once).
    fn stop_running(&mut self, agent: &str, fragment: &str, id: &str) {
        let t = self.state.turns.get_mut(id).expect("a running turn is held");
        assert!(t.active(), "only a running turn is stopped so: {id} is {:?}", t.phase);
        if t.stop_requested {
            return;
        }
        t.stop_requested = true;
        // a Stop is its question's answer too (Relay says `skip` to
        // a clarify waiting on words): the next message is a turn
        t.asking = false;
        let open: Vec<String> = t.prompts.iter().filter(|p| !p.closed).map(|p| p.id.clone()).collect();
        for p in t.prompts.iter_mut() {
            p.closed = true;
        }
        if t.phase == Phase::Waiting {
            t.phase = Phase::Running;
        }
        self.dirty = true;
        crate::ev!("turn.stopping", { "turn": id });
        for p in open {
            self.post(agent, fragment, records::WORK, records::work_id(id, &format!("pc:{p}")), records::turn_prompt_closed(id, &p, Closed::Stopped, None), Vec::new());
        }
        self.tell_runtime(id, Command::Stop { turn: id.to_string() });
    }

    /// An answer to a turn's prompt, from the agent's owner. Words go with
    /// it only for an option answered in words (the runtime asks for them
    /// otherwise, as its question in words). An answer naming its card (the
    /// chat's page names its agent and turn) is this agent's only for its
    /// own turn: another agent of the chat may show a card of the same
    /// prompt id, and a click on that card must never answer this one.
    #[allow(clippy::too_many_arguments)]
    fn answer(&mut self, agent: &Agent, fragment: &str, principal: &str, prompt: &str, option: &str, text: Option<String>, card: Option<&records::Card>, seq: u64) {
        if let Some(c) = card.filter(|c| c.agent != agent.identity) {
            crate::ev!("prompt.ignored", { "agent": agent.fragment, "prompt": prompt, "why": "it answers another agent's card", "for": c.agent });
            return;
        }
        let asked = |t: &&Turn| t.agent == agent.fragment && t.fragment == fragment && card.is_none_or(|c| c.turn == t.id) && t.prompts.iter().any(|p| p.id == prompt);
        let found = self.state.turns.values().find(asked).map(|t| t.id.clone());
        let Some(id) = found else {
            crate::ev!("prompt.ignored", { "agent": agent.fragment, "prompt": prompt, "why": "no open turn asked it" });
            return;
        };
        if principal != agent.owner {
            crate::ev!("prompt.ignored", { "turn": id, "prompt": prompt, "why": "only the agent's owner answers" });
            return;
        }
        let p = self.state.turns[&id].prompts.iter().find(|p| p.id == prompt).expect("found");
        if p.closed || p.expires_at <= self.now {
            crate::ev!("prompt.ignored", { "turn": id, "prompt": prompt, "why": "already closed: the first answer won" });
            return;
        }
        if !p.options.iter().any(|o| o == option) {
            crate::ev!("prompt.ignored", { "turn": id, "prompt": prompt, "why": "not one of its options" });
            return;
        }
        let words = text.filter(|_| p.words.iter().any(|w| w == option));
        self.answered(&agent.fragment, fragment, &id, prompt, option, principal, words.clone(), words, None, seq);
    }

    /// An open prompt of turn `id` answered with `option` by `principal`:
    /// closed, its card saying `shown` (the words, typed on it or sent as a
    /// message), the runtime given `words` and the files `attached` once
    /// the closure is on `work` (`fenced`); the turn runs on once none of
    /// its prompts is open. The closure names no `answerSeq`: a card
    /// answered live never starts a fresh turn (cards.rs), however far back
    /// a `/data` goes, and the runtime acts on no answer the journal does
    /// not hold.
    #[allow(clippy::too_many_arguments)]
    fn answered(&mut self, agent: &str, fragment: &str, id: &str, prompt: &str, option: &str, principal: &str, shown: Option<String>, words: Option<String>, attached: Option<Attached>, seq: u64) {
        let now = self.now;
        let closed = records::turn_prompt_closed(id, prompt, Closed::Answered, Some((option, principal, shown.as_deref())));
        let answer = Command::Answer { turn: id.to_string(), prompt: prompt.to_string(), option: Some(option.to_string()), seq, by: principal.to_string(), words: words.clone(), attached };
        let t = self.state.turns.get_mut(id).expect("an open turn's");
        let p = t.prompts.iter_mut().find(|p| p.id == prompt).expect("its prompt");
        assert!(!p.closed, "a prompt is answered once");
        p.closed = true;
        p.held = Some(Held { closed: closed.clone(), answer });
        if t.prompts.iter().all(|p| p.closed) && t.phase == Phase::Waiting {
            t.phase = Phase::Running;
        }
        t.last_ms = now;
        self.dirty = true;
        crate::ev!("prompt.answered", { "turn": id, "prompt": prompt, "option": option, "words": words.is_some() });
        self.fence(agent, fragment, id, prompt, closed);
    }

    fn fence(&mut self, agent: &str, fragment: &str, turn: &str, prompt: &str, body: Value) {
        self.fencing.insert((turn.to_string(), prompt.to_string()));
        let id = records::work_id(turn, &format!("pc:{prompt}"));
        self.out.push(Effect::Fence { agent: agent.to_string(), fragment: fragment.to_string(), turn: turn.to_string(), prompt: prompt.to_string(), id, body });
    }

    /// The platform's answer to a live answer's closure: on `work` (or a
    /// replay of it), the runtime has the answer; another body under its id
    /// (409) or a chat the agent may no longer post in, its wait ends with
    /// no answer, as at an expiry; no answer at all, it is posted again at
    /// the next tick, the answer held meanwhile, which is no sign of life: a
    /// platform that answers none for a turn's idle bound ends the turn,
    /// the answer never acted on.
    fn fenced(&mut self, turn: &str, prompt: &str, answer: ClaimAnswer) {
        self.fencing.remove(&(turn.to_string(), prompt.to_string()));
        let now = self.now;
        let Some(t) = self.state.turns.get_mut(turn).filter(|t| t.active() && t.prompts.iter().any(|p| p.id == prompt && p.held.is_some())) else {
            crate::ev!("fence.stale", { "turn": turn, "prompt": prompt, "why": "no running turn holds its answer" });
            return;
        };
        if answer != ClaimAnswer::Unanswered {
            t.last_ms = now;
        }
        let p = t.prompts.iter_mut().find(|p| p.id == prompt).expect("found");
        let answer = match answer {
            ClaimAnswer::Unanswered => {
                crate::ev!("fence.unanswered", { "turn": turn, "prompt": prompt });
                return;
            }
            ClaimAnswer::Ours { .. } => p.held.take().expect("held").answer,
            ClaimAnswer::Theirs | ClaimAnswer::Refused => {
                p.held = None;
                crate::ev!("fence.refused", { "turn": turn, "prompt": prompt, "why": "its closure is not this answer's to post" });
                Command::Answer { turn: turn.to_string(), prompt: prompt.to_string(), option: None, seq: 0, by: String::new(), words: None, attached: None }
            }
        };
        self.tell_runtime(turn, answer);
    }

    // ---- the runtime ----

    fn runtime(&mut self, from: &str, event: Event) {
        assert!(self.runtimes.contains_key(from), "an event comes from a runtime the bridge runs: {from}");
        let turn_of = |e: &Event| -> Option<String> {
            match e {
                Event::Draft { turn, .. } | Event::Reply { turn, .. } | Event::Attachment { turn, .. } | Event::Retract { turn, .. } | Event::Interim { turn, .. } => Some(turn.clone()),
                Event::Called { turn, .. } | Event::Step { turn, .. } | Event::Prompt { turn, .. } | Event::Asked { turn } | Event::End { turn, .. } => Some(turn.clone()),
                Event::Connected(_) | Event::Working(_) | Event::Say { .. } | Event::Notice { .. } => None,
            }
        };
        if let Some(id) = turn_of(&event) {
            if self.handed.get(&id).map(String::as_str) != Some(from) {
                crate::ev!("runtime.stale", { "turn": id, "runtime": from, "why": "no turn this life handed to that runtime" });
                return;
            }
            match self.state.turns.get_mut(&id) {
                Some(t) if t.active() => t.last_ms = self.now,
                _ => {
                    crate::ev!("runtime.stale", { "turn": id, "why": "the bridge holds no running turn by that id" });
                    return;
                }
            }
        }
        match event {
            Event::Working(working) => self.runtimes.get_mut(from).expect("checked").working = working,
            Event::Connected(connected) => {
                let r = self.runtimes.get_mut(from).expect("checked");
                if connected != r.connected {
                    crate::ev!("runtime.connected", { "runtime": from, "connected": connected });
                }
                r.connected = connected;
                self.pump_all();
            }
            Event::Draft { turn, text } => {
                let t = &self.state.turns[&turn];
                let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
                self.draft(&agent, &fragment, &turn, Some(records::cut_bytes(&text, limits::DRAFT_TEXT_MAX_BYTES)));
            }
            Event::Interim { turn, part } => {
                if self.state.turns[&turn].open.as_ref().is_some_and(|o| o.part == part) {
                    self.seal(&turn, true);
                }
            }
            Event::Reply { turn, part, text } => self.reply(&turn, part, Some(text), None),
            Event::Attachment { turn, part, file } => self.reply(&turn, part, None, Some(file)),
            Event::Retract { turn, part } => {
                let t = self.state.turns.get_mut(&turn).expect("checked");
                if t.open.as_ref().is_some_and(|o| o.part == part) {
                    t.open = None;
                    let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
                    self.draft(&agent, &fragment, &turn, None);
                }
            }
            Event::Called { turn, call } => {
                // its intent, on `work` before the call runs (as soon as the
                // lane posts it: the runtime goes on once told it was heard),
                // so a turn cut while it ran is told of it (note.rs). Asked
                // is all it says: no reply is sealed, nothing is shown run.
                let t = self.state.turns.get_mut(&turn).expect("checked");
                t.calls = t.calls.saturating_add(1);
                self.dirty = true;
                if t.calls <= limits::STEPS_PER_TURN_MAX && records::valid_post_id(&call.response) && records::valid_post_id(&call.tool_call_id) {
                    let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
                    self.post(&agent, &fragment, records::WORK, records::call_id(&turn, &call.response, call.position), records::turn_call(&turn, &call), Vec::new());
                }
            }
            Event::Step { turn, step } => {
                // the words before a tool call are a reply, posted before
                // its step; the turn works on after them, so they are
                // interim (its push leaves them out), unless the call only
                // updates its memory: an answer said beside housekeeping is
                // the turn's answer (Hermes sends nothing after it)
                self.seal(&turn, step.category != records::Category::Memory);
                let t = self.state.turns.get_mut(&turn).expect("checked");
                t.steps = t.steps.saturating_add(1);
                self.dirty = true;
                if t.steps <= limits::STEPS_PER_TURN_MAX {
                    let (agent, fragment, n) = (t.agent.clone(), t.fragment.clone(), t.steps);
                    self.post(&agent, &fragment, records::WORK, records::work_id(&turn, &n.to_string()), records::turn_step(&turn, n, &step), Vec::new());
                }
            }
            Event::Prompt { turn, prompt, text, options, ttl_ms } => self.prompt(&turn, prompt, text, options, ttl_ms),
            Event::Asked { turn } => {
                // the question shows before the answer it waits for, and the
                // chat is told whose answer it waits for (`turn.asked`, once
                // a question: the part asking it is in its id)
                self.seal(&turn, false);
                // an ask's turn (one with a return) asks back an agent, whose
                // words never answer it (`said`: hop 0's alone), so it would
                // wait out its bound: it is told at once, in words (its
                // runtime's question wants words: a clarify lets go of
                // nothing else), that no answer is coming, and goes on to
                // answer as best it can. A plain hand-off waits as before.
                let t = &self.state.turns[&turn];
                let answers = self.who_answers(t);
                if t.ret.is_some() {
                    crate::ev!("turn.asked_back", { "turn": turn });
                    let (by, by_name) = (t.asker.clone(), t.asker_name.clone());
                    self.tell_runtime(&turn, Command::Tell { turn: turn.clone(), seq: ASKED_BACK_SEQ, by, by_name, text: ASKED_BACK.into(), attached: None });
                    return;
                }
                let t = self.state.turns.get_mut(&turn).expect("checked");
                if !t.asking {
                    t.asking = true;
                    self.dirty = true;
                    crate::ev!("turn.asked", { "turn": turn });
                    let (agent, fragment, id, body) = (t.agent.clone(), t.fragment.clone(), records::work_id(&turn, &format!("q{}", t.last_part)), records::turn_asked(&turn, &answers));
                    self.post(&agent, &fragment, records::WORK, id, body, Vec::new());
                }
            }
            Event::End { turn, outcome } => self.end(&turn, outcome, Closed::Expired),
            Event::Say { agent, fragment, text, files } => {
                if self.agent(&agent).is_none() || (text.trim().is_empty() && files.is_empty()) {
                    return;
                }
                self.state.said += 1;
                self.dirty = true;
                let turn = records::said_turn_id(&agent, &self.life, self.state.said);
                crate::ev!("said", { "agent": agent, "fragment": fragment, "turn": turn });
                self.post(&agent, &fragment, records::CHAT, records::reply_id(&turn, 1), records::reply(&text, &turn, &[], 0, false), files);
            }
            Event::Notice { agent, fragment, category, text } => self.notice(&agent, &fragment, category, &text),
        }
    }

    /// A notice of the runtime's in a chat (`Event::Notice`): a `turn.notice`
    /// of the agent's turn running there, or of the last one this life handed
    /// there (a review its runtime finishes after the turn's end is about
    /// that turn), at most `NOTICES_PER_TURN_MAX` a turn, numbered from 1
    /// (`n<k>`). With no turn of this life's there (the agent's last ran in
    /// an earlier life, or none did), it is no turn's, and goes unposted.
    fn notice(&mut self, agent: &str, fragment: &str, category: NoticeCategory, text: &str) {
        if self.agent(agent).is_none() || text.trim().is_empty() {
            return;
        }
        let Some(at) = self.noticed.iter().position(|n| n.agent == agent && n.fragment == fragment) else {
            crate::ev!("notice.dropped", { "agent": agent, "fragment": fragment, "why": "no turn of this life's in that chat" });
            return;
        };
        let mut n = self.noticed.remove(at).expect("found");
        if n.notices >= limits::NOTICES_PER_TURN_MAX {
            crate::ev!("notice.dropped", { "turn": n.turn, "why": "a turn posts at most so many" });
            self.noticed.push_back(n);
            return;
        }
        n.notices += 1;
        let (turn, k) = (n.turn.clone(), n.notices);
        self.noticed.push_back(n);
        if let Some(t) = self.state.turns.get_mut(&turn).filter(|t| t.active()) {
            t.last_ms = self.now;
        }
        crate::ev!("notice.posted", { "turn": turn, "n": k, "category": category.as_str() });
        self.post(agent, fragment, records::WORK, records::work_id(&turn, &format!("n{k}")), records::turn_notice(&turn, category, text), Vec::new());
    }

    /// Reply `part` of a turn changed: its text, or a file it carries.
    fn reply(&mut self, id: &str, part: u32, text: Option<String>, file: Option<LocalFile>) {
        let t = &self.state.turns[id];
        if part == 0 || part <= t.last_part {
            crate::ev!("reply.late", { "turn": id, "part": part });
            return;
        }
        if t.open.as_ref().is_some_and(|o| o.part > part) {
            crate::ev!("reply.late", { "turn": id, "part": part });
            return;
        }
        if t.open.as_ref().is_some_and(|o| o.part != part) {
            self.seal(id, false);
        }
        let t = self.state.turns.get_mut(id).expect("checked");
        let open = t.open.get_or_insert_with(|| OpenReply { part, ..OpenReply::default() });
        if let Some(text) = text {
            open.text = text;
        }
        if let Some(file) = file {
            if open.files.len() < limits::ATTACHMENTS_MAX {
                open.files.push(file);
            } else {
                crate::ev!("attachment.dropped", { "turn": id, "why": "a reply carries at most so many" });
            }
        }
        let shown = records::cut_bytes(&open.text, limits::DRAFT_TEXT_MAX_BYTES);
        let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
        if !shown.trim().is_empty() {
            self.draft(&agent, &fragment, id, Some(shown));
        }
    }

    /// Posts the turn's open reply, if it says anything: `interim` when
    /// the turn works on after it (a tool call follows its words).
    fn seal(&mut self, id: &str, interim: bool) {
        let view = self.views.get(&self.state.turns[id].fragment).cloned().unwrap_or_default();
        let t = self.state.turns.get_mut(id).expect("sealing a held turn");
        let Some(open) = t.open.take() else { return };
        t.last_part = t.last_part.max(open.part);
        let empty = open.text.trim().is_empty() && open.files.is_empty();
        // interim replies have a bound of their own, so narration never
        // spends what the turn's answer and questions may say
        let spent = if interim { t.interim >= limits::INTERIM_REPLIES_PER_TURN_MAX } else { t.replies - t.interim >= limits::REPLIES_PER_TURN_MAX };
        if empty || spent {
            if spent && !empty {
                crate::ev!("reply.dropped", { "turn": id, "interim": interim, "why": "a turn posts at most so many" });
            }
            return;
        }
        t.replies += 1;
        if interim {
            t.interim += 1;
        }
        // what an asked turn says, its answer carries back (its interim
        // words are narration, not the answer)
        if let Some(r) = t.ret.as_mut().filter(|_| !interim) {
            r.files = r.files.saturating_add(u32::try_from(open.files.len()).expect("a reply's files are bounded"));
            if !open.text.trim().is_empty() && r.said.len() < limits::ANSWER_TEXT_MAX_BYTES {
                if !r.said.is_empty() {
                    r.said.push_str("\n\n");
                }
                // no control characters: an answer's body stays within the
                // platform's bound once its text is escaped
                r.said.extend(open.text.trim().chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')));
                r.said = records::cut_bytes(&r.said, limits::ANSWER_TEXT_MAX_BYTES);
            }
        }
        self.dirty = true;
        let (agent, fragment, n, hop) = (t.agent.clone(), t.fragment.clone(), t.replies, t.hop);
        // A reply that names another agent of the chat hands off to it.
        let words = records::mentions(&open.text);
        let to: Vec<String> = self.agents.iter().filter(|a| a.fragment != agent && view.agents.contains(&a.identity) && words.iter().any(|w| answers_to(a, w))).map(|a| a.identity.clone()).collect();
        let body = records::reply(&open.text, id, &to, hop + 1, interim);
        crate::ev!("reply.posted", { "turn": id, "n": n, "files": open.files.len(), "to": to.len(), "interim": interim });
        self.post(&agent, &fragment, records::CHAT, records::reply_id(id, n), body, open.files);
        self.draft(&agent, &fragment, id, None);
    }

    fn prompt(&mut self, id: &str, prompt: String, text: String, options: Vec<PromptOption>, ttl_ms: Option<u64>) {
        let t = &self.state.turns[id];
        let valid = records::valid_token(&prompt, 64)
            && !options.is_empty()
            && options.len() <= limits::PROMPT_OPTIONS_MAX
            && options.iter().all(|o| records::valid_token(&o.id, 32) && !o.label.trim().is_empty())
            && t.prompts.len() < limits::PROMPTS_PER_TURN_MAX;
        let repeat = t.prompts.iter().any(|p| p.id == prompt);
        if repeat {
            return;
        }
        if !valid {
            // A prompt no card can show is expired at once, so the runtime
            // never waits on it.
            crate::ev!("prompt.refused", { "turn": id, "why": "not a prompt a card can show" });
            self.tell_runtime(id, Command::Answer { turn: id.to_string(), prompt, option: None, seq: 0, by: String::new(), words: None, attached: None });
            return;
        }
        self.seal(id, false);
        // A runtime's own lifetime is held within bounds; the operator's
        // setting is theirs (tests run it short).
        let ttl = match ttl_ms {
            Some(t) => t.clamp(limits::PROMPT_TTL_MS_MIN, limits::PROMPT_TTL_MS_MAX),
            None => self.settings.prompt_ttl_ms,
        };
        let expires_at = self.now + ttl;
        let t = self.state.turns.get_mut(id).expect("checked");
        let Some(owner) = self.agents.iter().find(|a| a.fragment == t.agent).map(|a| a.owner.clone()) else { return };
        t.prompts.push(Prompt { id: prompt.clone(), options: options.iter().map(|o| o.id.clone()).collect(), words: options.iter().filter(|o| o.words).map(|o| o.id.clone()).collect(), expires_at, closed: false, held: None });
        t.phase = Phase::Waiting;
        self.dirty = true;
        let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
        crate::ev!("prompt.asked", { "turn": id, "prompt": prompt, "expiresAt": expires_at });
        self.post(&agent, &fragment, records::WORK, records::work_id(id, &format!("p:{prompt}")), records::turn_prompt(id, &prompt, &text, &options, &owner, expires_at), Vec::new());
    }

    /// The run is over: seal its reply and owe its end, then pump the chat.
    /// Unexpired cards outlive the run, except an explicit Stop. Their
    /// later answers are reconstructed from the journal as fresh turns.
    fn end(&mut self, id: &str, outcome: Outcome, unanswered: Closed) {
        self.seal(id, false);
        let t = self.state.turns.get_mut(id).expect("ending a held turn");
        assert!(t.active(), "a turn's run ends once: {id} is {:?}", t.phase);
        // a live answer still held goes with it: the runtime never had it
        let prompts = std::mem::take(&mut t.prompts);
        let end = Owed { id: records::work_id(id, "end"), body: records::turn_end(id, &outcome), to: None };
        t.phase = Phase::Ended;
        t.owed = vec![end.clone()];
        // over, it asks nothing: its asker's next message is a turn
        t.asking = false;
        let (agent, fragment, hop) = (t.agent.clone(), t.fragment.clone(), t.hop);
        self.dirty = true;
        // its replies may be read after it is let go: one hop past it
        self.ended.push_back(EndedTurn { id: id.to_string(), agent: agent.clone(), fragment: fragment.clone(), hop, at: self.now });
        while self.ended.len() > limits::ENDED_HOPS_MAX {
            self.ended.pop_front();
        }
        crate::ev!("turn.end", { "turn": id, "agent": agent, "fragment": fragment, "outcome": match &outcome { Outcome::Idle => "idle", Outcome::Stopped => "stopped", Outcome::Error(_) => "error" } });
        self.fencing.retain(|(t, _)| t != id);
        let now = self.now;
        for p in prompts.iter().filter(|p| !p.closed && (outcome == Outcome::Stopped || p.expires_at <= now)) {
            self.post(&agent, &fragment, records::WORK, records::work_id(id, &format!("pc:{}", p.id)), records::turn_prompt_closed(id, &p.id, unanswered, None), Vec::new());
        }
        self.owe(&agent, &fragment, id, end);
        self.owe_answer(id, &outcome, false);
        self.draft(&agent, &fragment, id, None);
        self.pump(&agent, &fragment);
    }

    // ---- time ----

    fn tick(&mut self) {
        let now = self.now;
        let mut expired: Vec<(String, String)> = Vec::new();
        let mut quiet: Vec<String> = Vec::new();
        for t in self.state.turns.values() {
            for p in t.prompts.iter().filter(|p| !p.closed && p.expires_at <= now) {
                expired.push((t.id.clone(), p.id.clone()));
            }
            // a turn asking its person waits as long as a prompt would
            let bound = if t.asking { self.settings.turn_idle_ms.max(self.settings.prompt_ttl_ms) } else { self.settings.turn_idle_ms };
            let idle = t.phase == Phase::Running && now.saturating_sub(t.last_ms) > bound;
            if idle {
                quiet.push(t.id.clone());
            }
        }
        for (id, prompt) in expired {
            let t = self.state.turns.get_mut(&id).expect("listed");
            t.prompts.iter_mut().find(|p| p.id == prompt).expect("listed").closed = true;
            if t.prompts.iter().all(|p| p.closed) && t.phase == Phase::Waiting {
                t.phase = Phase::Running;
            }
            t.last_ms = now;
            self.dirty = true;
            let (agent, fragment) = (t.agent.clone(), t.fragment.clone());
            crate::ev!("prompt.expired", { "turn": id, "prompt": prompt });
            self.post(&agent, &fragment, records::WORK, records::work_id(&id, &format!("pc:{prompt}")), records::turn_prompt_closed(&id, &prompt, Closed::Expired, None), Vec::new());
            self.tell_runtime(&id, Command::Answer { turn: id.clone(), prompt, option: None, seq: 0, by: String::new(), words: None, attached: None });
        }
        for id in quiet {
            self.tell_runtime(&id, Command::Forget { turn: id.clone() });
            self.end(&id, Outcome::Error("the agent stopped answering".into()), Closed::Expired);
        }
        // a live answer whose closure was not answered: posted again
        let unfenced: Vec<(String, String, String, String, Value)> = self
            .state
            .turns
            .values()
            .filter(|t| t.active())
            .flat_map(|t| t.prompts.iter().filter_map(|p| p.held.as_ref().map(|h| (t.agent.clone(), t.fragment.clone(), t.id.clone(), p.id.clone(), h.closed.clone()))))
            .filter(|(_, _, turn, prompt, _)| !self.fencing.contains(&(turn.clone(), prompt.clone())))
            .collect();
        for (agent, fragment, turn, prompt, body) in unfenced {
            self.fence(&agent, &fragment, &turn, &prompt, body);
        }
        // A claim left unanswered (no answer, or the computer held) is
        // claimed again.
        self.pump_all();
        // a turn no longer running has nothing more to say to its runtime
        let state = &self.state;
        self.handed.retain(|id, _| state.turns.get(id).is_some_and(Turn::active));
    }

    fn gone(&mut self, agent: &str, fragment: &str) {
        let ids: Vec<String> = self.state.turns.values().filter(|t| t.agent == agent && t.fragment == fragment).map(|t| t.id.clone()).collect();
        for id in ids {
            crate::ev!("turn.dropped", { "turn": id, "why": "the agent is no longer in this fragment" });
            if self.state.turns[&id].active() {
                self.tell_runtime(&id, Command::Forget { turn: id.clone() });
            }
            self.let_go(&id, &format!("it is no longer in {fragment}"));
        }
        self.noticed.retain(|n| !(n.agent == agent && n.fragment == fragment));
        self.views.remove(fragment);
    }
}

/// Whether `agent` answers to `@word`: its name, or its fragment's label.
fn answers_to(agent: &Agent, word: &str) -> bool {
    agent.name.to_ascii_lowercase() == word || label(&agent.fragment).to_ascii_lowercase() == word
}

/// A loaded state agrees with itself, or is corrupt.
pub fn check(state: &State) -> Result<(), Corrupt> {
    if !STATE_VERSIONS.contains(&state.version) {
        return Err(Corrupt(format!("state version {} is none of {STATE_VERSIONS:?}", state.version)));
    }
    if state.version < version_of(state) {
        return Err(Corrupt(format!("state version {} holds an ask's return", state.version)));
    }
    if state.turns.len() > limits::TURNS_OPEN_MAX {
        return Err(Corrupt(format!("{} open turns is past the bound", state.turns.len())));
    }
    for (id, t) in &state.turns {
        if &t.id != id {
            return Err(Corrupt(format!("turn {id} holds the id {}", t.id)));
        }
        let own_id = match &t.continues {
            Some(cut) => records::continuation_id(cut),
            None => records::turn_id(&t.agent, &t.cause.fragment, &t.cause.channel, t.cause.seq),
        };
        if own_id != t.id {
            return Err(Corrupt(format!("turn {id} is not its cause's, nor its cut turn's continuation")));
        }
        if t.continues.is_some() && (t.continue_after || t.ret.is_some()) {
            return Err(Corrupt(format!("turn {id} continues a cut one and is continued or answers an ask")));
        }
        let cursor = state.cursors.get(&cursor_key(&t.agent, &t.cause.fragment, &t.cause.channel)).copied().unwrap_or(0);
        if cursor < t.cause.seq {
            return Err(Corrupt(format!("turn {id} is past its channel's cursor {cursor}")));
        }
        if t.order == 0 || t.order > state.admitted {
            return Err(Corrupt(format!("turn {id}'s order {} is not an admission", t.order)));
        }
        if t.phase == Phase::Waiting && t.prompts.iter().all(|p| p.closed) {
            return Err(Corrupt(format!("turn {id} waits on no prompt")));
        }
        if t.asking && !t.active() {
            return Err(Corrupt(format!("turn {id} is {:?} and asks its asker", t.phase)));
        }
        if t.interim > t.replies {
            return Err(Corrupt(format!("turn {id} posted {} interim replies of {}", t.interim, t.replies)));
        }
        if t.note.as_ref().is_some_and(|n| n.len() > limits::NOTE_MAX_BYTES) {
            return Err(Corrupt(format!("turn {id}'s note is past a note's bound")));
        }
        let ended = t.phase == Phase::Ended;
        if ended == t.owed.is_empty() {
            return Err(Corrupt(format!("turn {id} is {:?} and owes {} records", t.phase, t.owed.len())));
        }
        if t.owed.len() > 3 {
            return Err(Corrupt(format!("turn {id} owes {} records: at most its start, its end and its answer", t.owed.len())));
        }
        let own = format!("wk:{id}:");
        let ours = |o: &Owed| match &o.to {
            None => o.id.starts_with(&own),
            Some(to) => o.id == records::answer_id(id) && t.ret.as_ref().is_some_and(|r| &r.agent == to),
        };
        if let Some(o) = t.owed.iter().find(|o| !ours(o) || o.body["turn"] != id.as_str()) {
            return Err(Corrupt(format!("turn {id} owes {}, not one of its own", o.id)));
        }
        if t.routine && t.hop != 0 {
            return Err(Corrupt(format!("turn {id} is a routine at hop {}", t.hop)));
        }
        if let Some(r) = &t.ret {
            let whole = records::fragment_name_ok(&r.agent) && records::fragment_name_ok(&r.chat) && !r.name.trim().is_empty();
            if !whole || r.said.len() > limits::ANSWER_TEXT_MAX_BYTES {
                return Err(Corrupt(format!("turn {id}'s return is not one an ask makes")));
            }
        }
    }
    if state.agent_turns.len() > limits::AGENT_TURN_CHATS_MAX {
        return Err(Corrupt(format!("{} chats' agent turns is past the bound", state.agent_turns.len())));
    }
    for (chat, times) in &state.agent_turns {
        if times.is_empty() || times.len() > limits::AGENT_TURNS_PER_CHAT_MAX || !times.windows(2).all(|w| w[0] <= w[1]) {
            return Err(Corrupt(format!("chat {chat}'s agent turns are not 1 to {} times in order", limits::AGENT_TURNS_PER_CHAT_MAX)));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
