# The bridge

`images/bridge`: the one process a computer image runs between its agent
runtimes and the fragment API (docs/computers.md). It owns nothing (lesson
1): a chat's `chat` channel owns what was said and its `work` channel
which turns started, the runtime owns the turn in flight, the Computer DO
owns the computer. The bridge keeps a cursor per followed channel and the
turns it admitted that have not ended, and writes docs/chat-records.md's
records. Its rules are listed once, in `engine.rs`'s header; every bound
is a const in `limits.rs`, with its reason.

Two runtimes ship (`runtime/`): `relay`, the connector Hermes' Relay
dials (below), and `script`, a deterministic agent (the stub image's), so
the platform's lanes run with no agent runtime. Another runtime is
another module there, or a bridge of its own: nothing the platform does
depends on which runs.

## Which runtime runs an agent

An agent runs on the runtime its `agent.json` `runtime` names, `hermes`
when it names none (`runtime::DEFAULT`). `BRIDGE_RUNTIME` is the module
run for `hermes`. A bridge runs at most `RUNTIMES_MAX`.

- **Read with the computer**: at start (the bridge waits for every
  agent's file, as it waits for the computer), then with every read of
  it. A file that does not read keeps what it named last; an agent first
  seen mid-life whose file does not read is left out until it does.
- **Each runtime holds only its own agents.** A turn is claimed only
  while its agent's runtime can take one (`Connected`); any runtime's
  `Working` holds the computer; its menu is its agents' commands. A turn
  stays with the runtime it was handed to, kept in memory only.
- **A runtime the bridge does not run is refused, never substituted**:
  each turn gets both its records, its end saying "this computer doesn't
  run <runtime> agents", and an ask's answer says so too.
- A runtime that ends ends the bridge.

## The routes it calls

Their shapes are docs/api.md's; `api.rs` has one method per route. Each
acts as one of the computer's agents (`x-fragment-agent`), but the
computer's own:

- the computer's: `GET /api/computer` (`?simplex=1` from an image that
  names a SimpleX core), `POST /api/computer/door`, and the WebSockets
  `GET /api/computer/keepalive` and `GET /api/computer/relay`;
- what an agent follows: `GET /api/fragments`, and on each fragment
  `GET …/channels`, `…/channels/{c}`, `…/members` and `…/subscriptions`
  (`POST` one when it has none), `GET /f/{f}/__people`, and the
  `/f/{f}/__live` socket;
- what it writes: `POST …/channels/chat`, `…/channels/work` and its own
  `…/channels/tasks` (an ask's answer), `PUT …/channels/chat/draft`,
  `PUT` and `GET …/blobs/{sha256}`;
- for its owner: `GET` and `POST /api/fragments?for=` (its home, its
  SimpleX chat), and on its own fragment `GET …/status` (its name) and
  `GET …/file?path=agent.json` (its home and runtime).

The script runtime also calls its own fragment's `…/schedules?for=`, and
a connector's `…/door?for=` (the lanes' refused bind). `hermes-boot`
calls its own fragment's `…/files`, `…/file` and `POST …/files`, and its
owner's skills' `…/files?for=` and `…/file?for=`.

## Settings

`fragment-bridge run` reads environment variables; `main.rs` has each
with its default. What an image sets:

- `FRAGMENT_API`, required; `RESTORE_PENDING=1` to wait for
  `BRIDGE_RESTORED` before anything else (docs/computers.md).
- `BRIDGE_HOLD` and `BRIDGE_HELD`, the platform's hold before a save
  (docs/computers.md, "The hold"). The bridge claims no turn while the
  hold exists, and answers it only while no claim is in flight, so a save
  never misses a claim that lands after it. `BRIDGE_HELD_LEAVE_OUT`: what
  the save may leave out. An image with more to quiet names another file
  and answers the platform itself.
- `BRIDGE_RUNTIME`: `relay` (with `BRIDGE_RELAY_SECRET_FILE`, the secret
  Hermes is given too) or `script`.
- `BRIDGE_AGENTS_FILE`: the agents the image has made ready (`ready.rs`).
  Unset, every agent the platform lists runs.
- `BRIDGE_SCREEN_LISTEN`, `BRIDGE_SCREENS_FILE` and `BRIDGE_SCREEN_START`:
  each agent's screen (`screen.rs`, `screens.rs`, `lease.rs`).
- `BRIDGE_SIMPLEX_BIN`: SimpleX's core (below). Unset, nothing of SimpleX
  runs.
- `BRIDGE_CLAUDE_BIN`: the person's own unmodified `claude` (below, "The
  Claude runtime"), run as `BRIDGE_CLAUDE_USER` (`uid:gid`, never root),
  its config, work and state under `/data`. Unset, as in every image
  today, an agent whose `agent.json` names `claude` has its turns refused,
  saying so.

## State, and what it never does twice

Its state (`state.json`, written whole before a step's effects) is a
cache. The authority on which turns started is the chat's `work`
channel, so `/data` restored from any earlier save, or lost, costs reads
and runs nothing twice. A state of another format is set aside for a
fresh one; one that contradicts itself is refused.

- **One life per turn.** Each bridge process is a life: 128 random bits,
  never written to `/data`. A turn runs only in the life whose claim (its
  `turn.start`, naming the life) the platform answered as its own. A 409
  is another life's claim, and the turn ends as lost. A 403 or 404 (the
  agent may not post on `work`) drops it, recorded nowhere. No answer
  runs nothing, and the turn is claimed again. So a turn runs at most
  once; one whose life ended first is lost, and the chat is told.
- **A claim waits** for the runtime (`Connected`) and for no hold, so a
  turn waits unclaimed, for whichever life comes next.
- **Every turn has both records**, its start and its end, including a
  refused turn and one stopped while it waited. A turn leaves the state
  only once both are answered. A life that ends first leaves them to the
  next, which posts them again under the same ids and bodies.
- **An answer in words goes only to its turn's life** (`Tell`, `Answer`).
  A restart ends that turn as lost, so the next life reads the message as
  a turn of its own.

## Restarts

On SIGTERM the bridge exits within `SHUTDOWN_MS_MAX`. When it starts
again, every turn its state holds that is not queued was an earlier
life's. Each ends as lost (`lost when the computer restarted`, its
unexpired cards left answerable) and never runs again, though it may be
continued (below). Queued turns are claimed like any other.

### The turn after a cut one is told

A cut turn may have had effects, and Hermes keeps its message as the
session's last, joining the next message to it, so its model would redo
the cut request. Two parts close this:

- The agent's next turn in that chat carries a note (`note.rs`): the turn
  before was cut by a restart, check what it already did before doing it
  again, then what it was asked, each call it asked for that no step
  ended (it may or may not have run), its steps, cards and replies.
  Built from the chat's journal alone, it is the same in every life and
  said once. A journal that does not answer in time gives no note: a turn
  is never held back for one. Relay hands it as the inbound's `context`.
- Before the gateway starts, our image's `hermes-boot` closes each cut
  turn in its Hermes session (`hermes::CLOSE_CUT_TURNS`), so the next
  message is a user message of its own.

### A cut turn is continued once

A turn a restart cut is picked up again by itself, once, so nobody types
"continue" and nothing runs twice (`engine.rs`, `continue_turn`).

- **Which**: a turn this life ended as lost, once that end lands on
  `work` as this life's own. A life that ran the turn to its end posted
  that end first, so this one's is refused and nothing is continued.
- **Not continued**: an ask's turn (its asker is answered `lost`), a
  command, a turn its asker stopped (a Stop is never undone), one waiting
  on its person (their answer is its next turn), and a continuation: a
  cut continuation is told to the turn after it, as any cut turn is.
- **What it is**: a turn queued in the cut turn's place, with its cause,
  asker and depth, its id the cut turn's `records::continuation_id`, so
  its claim makes it one run in any life. The computer says it
  (`CONTINUE_TEXT`), never as its asker's words, and it carries the cut
  turn's note; the chat is told as it starts (`CONTINUE_NOTICE`). No
  runtime is asked to rerun a call.
- A state holding one is format 4, which an older bridge sets aside.

### A late message is told its age

Paul (2026-10-31): an agent handed a message late (a backlog after an
outage, a restart or a sleep) is told how old it is, and judges whether
it is still wanted; nothing is cut off. The engine keeps each turn's
cause time (`Turn::said_at`), and a turn handed at least
`MESSAGE_AGE_TOLD_MS` (5 minutes, five pre-wake holds: past an ordinary
claim, cold wake or queue) after it carries one plain line
(`TurnStart::age`, `note::age`), which the driver puts first in its
note, before a card's or a cut turn's: "This message was sent about 3
hours ago, and is only reaching you now. If it may no longer be wanted,
check before acting on it." A continuation is told the age of what its
cut turn answered. A routine (its message says its hour), a command,
and a state from before keep or tell none.

### What a rollback forgot

A computer can go back to an older save (docs/computers.md, "What its
owner is told"). The bridge then reads again the turns run since that
save, and each of their claims is a 409. Those 409s are exactly what its
runtime forgot. The agent's next turn in that chat is told what each
forgotten turn was asked and replied, from the journal
(`note::forgotten`), once a life.

## A card outlives its turn

Waiting on a person (a card, or a question in words) never holds the
computer, so a sleep or a restart may cut a turn under its card. The
turn ends as lost; its cards stay answerable until `expiresAt`. An owner's
answer that no running turn of this life holds is late, and the engine
resolves it from the chat's journal, read back to the card (`cards.rs`):

- **The first valid answer starts one fresh turn**, caused by the
  answer's record, so a retry after any restart is the same claim. Its
  message is the owner's answer; its note is the card's turn as `note.rs`
  renders a cut one. The old turn never resumes.
- **Only a late closure rebuilds.** A late answer's closure names it
  (`answerSeq`). A live one names none, and the runtime gets a live
  answer only once its closure is on `work`, so a rollback to before the
  answer finds it answered live and starts nothing.
- **Otherwise nothing runs**: a card closed already, its turn stopped
  first, another option or another person's answer. An answer read after
  the card's life closes it `expired`; a journal that cannot be read whole
  closes it `history_unavailable`. A failed read is read again, never a
  closure.

A card answered live whose turn a crash then cut is answered and lost, as
at-most-once has it; the next turn's note says so. Our image gives
Hermes' approval and clarify waits the card's life
(`BRIDGE_PROMPT_TTL_MS`), so a runtime alive at its expiry times out with
it.

## Runtime work outside a turn

The keepalive is held while a turn is queued or runs (not while it waits
on a person), and while a runtime says it has work of its own (`Working`):
in our image, delegated children still running after their parent
replied.

Our image's `fragment-activity` plugin holds
`ws://<BRIDGE_RELAY_LISTEN>/relay/activity` with the gateway's Relay
token. The bridge says `ready`, then answers each frame once its loop has
it, and the hook returns only then:

- `{"working": bool}` → `working`. True holds the computer until false,
  the socket's close, or 30 s silent. A socket opened again says its
  children's work first. Nothing of it is saved: a new process has none.
  Their results come back as ordinary Relay replies, a turn of the
  runtime's own.
- A call the model asked for, before Hermes runs any call of its
  response (Hermes' `post_api_request`): `{turn, response, call:
  {position, tool_call_id, tool, args}}` → `called`. It is the call's
  intent (docs/chat-records.md, `turn.call`), one record per turn,
  response and place (`records::call_id`), posted as soon as the lane
  can, so it may trail the call's start. `response` is the plugin's id
  for that response, given once. Hermes has no supported "call started"
  hook: `pre_tool_call` is a policy gate that blocks a slow hook.
- A tool call's end (Hermes' `post_tool_call`): `{turn, response, tool,
  args, excerpt, tool_call_id, status, duration_ms}` → `observed`, or
  `refused`. It is the turn's step (docs/chat-records.md, `turn.step`);
  `response` is the turn's latest, `args` Hermes' own preview of the call
  and `excerpt` a command's exit code, never its output. Only the person's
  turn's own calls are steps (`pre_llm_call` maps Hermes' run to our
  turn): its memory review's and a delegated child's are not. A call
  whose end is not observed is no step; nothing is guessed.

## Agents asking each other

A person's agents hand work to each other in a chat they share. One
agent names another with an `@name` in a reply (the bridge stamps its
`to`), or posts a message naming the other in `to` (`fragment ask`; our
image's `fragment_ask`). The bridge that answers decides whether this is
a turn:

- **It counts the hop** (`engine.rs`, `hop_of`). A record by an agent of
  this computer is one hop past the turn that agent is in. That is its
  turn in that chat, or the turn its `turn` names that ended within
  `ENDED_HOPS_MS`, or its deepest turn elsewhere, whichever is deepest.
  An agent in no turn counts as `HOPS_MAX`. A record's `hop` only raises
  the count, so a post made around the bridge (the CLI, the API) resets
  nothing. Another computer's agent is held to the hop it claims, at
  least 1. A person's agents all run on one computer (decision 13), so
  every hand-off between them meets this count.
- **A chat's budget.** Its agents start at most
  `AGENT_TURNS_PER_CHAT_MAX` turns of each other in
  `AGENT_TURNS_WINDOW_MS`, counted by the causing records' times and kept
  in the state. Past the budget a hand-off is refused, its end saying
  why. A person's message never counts.
- **An answer comes back** (docs/chat-records.md, "An ask"). An asked
  turn keeps where to return, and its end owes one more record: the
  answer, on the asking agent's own `tasks`, which that agent reads as a
  turn in the chat it asked from. The owed record is the delivery: it is
  retried under one id until the platform answers it, and nothing waits
  or is held awake for it. An asked agent taken off the computer still
  answers an asker that stays, also when the bridge first sees it gone at
  a start: a turn not over ends, its answer saying so (`LEFT`), and an
  answer not yet landed is kept. The asked agent's identity is kept with
  the turn (`Return::by`), so the answer is by it once it is gone (#722).
- **`tasks` is the agent's own.** A routine or `joined` starts anything
  only from the agent fragment's own key or its owner. Another of the
  owner's agents, which may post there, is passed over.
- **A routine that names no chat runs at home**, read as it fires
  (docs/api.md, "An agent's home"); a home that does not read is read
  again, so no fire is lost. A SimpleX chat named but not made yet is
  waited for up to `HOME_CHAT_WAIT_MS`, then the routine runs in the
  agent's own chat. An agent with no chat with its owner runs it nowhere.

The count lives here because the platform holds no chat record
(docs/cloudflare-v1.md, the rule), and only the bridge knows which turn
an agent is in.

## Commands

An agent's owner commands its runtime from the chat (docs/chat-records.md,
"A command"). A runtime names its menu (`Runtime::menu`): each command,
and how the bridge carries it (`How`). Only the agent's owner, and only
with the menu, reaches a runtime with a command. A message's leading `/`
never does (Relay: a zero-width space goes before it). The agent's first
turn in a chat in a life posts the menu on `work` (`commands`).

## SimpleX

An agent's owner may link the agent to their SimpleX app (decision 32;
`simplex/mod.rs` and `run.rs` say how). The bridge runs SimpleX's own
core, `simplex-chat`, one per linked agent, as its child process, over
the core's bot API on loopback.

- **Only where the image names the core** (`BRIDGE_SIMPLEX_BIN`). The
  bridge's reads of its computer say so (`?simplex=1`), so its owner's
  Settings offer SimpleX. And **only while the computer is always on**
  (its view's `alwaysOn`: docs/billing.md): a computer that sleeps would
  hear nothing, so it runs no core.
- **SimpleX is a chat of its own**: the agent's chat with its owner,
  labelled `<label>-simplex` and titled SimpleX (docs/api.md, "An
  agent's home"). Its name is chosen once and written to `link.json`
  before anything makes it, so across all lives that one chat is made,
  once. While linked, it mirrors their app both ways. What their contact
  sends is posted there as relayed records (docs/chat-records.md, "A
  relayed message"). What the agent says there goes to their app once
  posted: its replies but the interim ones, with their files, and a
  card's question with its choices. An unlink stops the mirror and keeps
  the chat; a link again is that chat. `state.json` holds nothing of
  SimpleX.
- **The agent is told in that chat alone**, as the chat's topic on every
  inbound there (Relay's `chat_topic`). Hermes reads a topic as a label,
  never as an instruction. Its name in their app is the agent's title,
  and follows a rename.
- **Only its owner links or unlinks it**, from their Settings (docs/api.md,
  "An agent's doors"). The link page, `/simplex/?agent=` on the screen
  port (`simplex/http.rs`), posts a ticket the platform signed with the
  agent's key. The bridge takes each ticket once, and only for this
  computer and that agent. The agent's tools reach that port too, and can
  neither link nor see an invitation.
- **A link's door key** is made for each link (0600, in no process's
  environment) and registered with the ticket (`POST
  /api/computer/door`). Every message the link carries is a relayed
  record that key signs. The platform takes no relayed record without its
  door's signature, so the agent's own key never speaks for its owner.
- **SMP on 5223; XFTP through the platform's relay.** Cloudflare
  Containers reset a TLS hello on 443 that names no server, and that
  missing name is how a SimpleX server on 443 knows a client speaks its
  protocol. XFTP's servers (files) are on 443 alone, so files go through
  `GET /api/computer/relay` (`simplex/socks.rs`).
- **The hold stops every core** before the bridge answers it, so a save
  takes their databases whole.
- **A start that went back in time** (`restored.rollback`) syncs the
  ratchet before anything is sent, never forcing a sync already accepted
  (v7.0.3's force makes new keys each time, which an offline phone cannot
  follow). A connection repeated forces already damaged may stay stuck.
  Restarts and recovery send the owner nothing.
- **The agent is kept out**: the core and `BRIDGE_SIMPLEX_DIR` are root's,
  0700, and on no agent's PATH. The agent's passwordless sudo still
  reaches root, and a core's bot API listens on loopback (the debt
  ledger, "SimpleX retains the computer's root trust and rewindable
  state").

## The Claude runtime

`runtime/claude.rs`: the person's own unmodified `claude`, signed in with
their own plan, driven over its stream-json wire as Anthropic's Agent SDK
drives it. The bridge never sits between it and Anthropic: no proxy, base
URL or key, and no `ANTHROPIC_*` or `CLAUDE_CODE_*` variable of anyone's
reaches it (`withheld`).

- **A session per agent and chat**, its id derived from the two and the
  chat's `/new` count, so every life names the same one. It resumes when
  its transcript is on disk, and its process stops once idle.
- **A turn** is one user message: its note, quote, asker and files in
  words. Text deltas are the draft, each text block a reply part, each
  `tool_use` its call's intent (`turn.call`: its assistant message the
  response, its place there), each tool's result a step naming that
  response, and its `result` its end (a sign-in or a plan's limit in
  Claude Code's own words). Claude Code writes a call's line before it
  runs it, but the tool may start before the bridge reads the line, so
  the intent trails its start by milliseconds, as Hermes' does.
- **No technical approvals:** tools run with permissions bypassed.
  `AskUserQuestion` alone asks: each question a card
  (`<tool_use_id>-q<n>`), its options and "Other" in words; an unanswered
  card refuses the call.
- **Stop** is Claude Code's interrupt. A turn the bridge lets go of is
  interrupted and its output dropped until its `result`, or its process
  replaced past `DRAIN_MS_DEFAULT`: one turn's words are never another's.
- **A crash** leaves the cut call open in the transcript. The cut turn is
  continued once, in the resumed session, which tells its model the
  call's outcome is unknown; the bridge's note names the call as one that
  may or may not have run (recorded with Haiku 5.5: it found the call
  still running and waited, never running it again).
- **Its menu:** `new` (the chat's next session) and `stop`.

## Hermes' Relay

Relay is the connector protocol of Hermes' gateway (decision 21); the
bridge is its connector (`runtime/relay/`). Hermes' side, as read from
v0.21.7's `gateway/relay/` (its code wins over its contract document;
the wire is v0.21.5's), contract version 1. Relay is marked
experimental: a Hermes upgrade reads it again, and the real-Hermes lane
(`tests/docker.rs`) runs against it.

- **The dial.** Hermes is given `GATEWAY_RELAY_URL` (it dials
  `<url>/relay`, `http` → `ws`), `GATEWAY_RELAY_ID` and
  `GATEWAY_RELAY_SECRET`, so it provisions nothing. Its token is `Bearer
  base64url_unpadded("{id}:{exp}:{hex(HMAC-SHA256(secret, "{id}:{exp}"))}")`,
  with `exp` five minutes on, fresh each dial. A refusal is close 4401:
  `expired` (it dials again) or `unauthorized` (after one handshake, it
  stops). Frames are JSON objects, one per line. Hermes pings, and
  reconnects with backoff.
- **Media**: `POST <url>/relay/media` (the bytes, `content-type`,
  `x-media-filename`) → `{id}`, and `GET <url>/relay/media/<id>`, behind
  the same token. At most 25 MiB.
- Hermes `hello {platform, botId}` → the connector's `descriptor`: the
  keys Hermes requires, and `supported_ops`. Draft streaming needs both
  `supports_draft_streaming` and `draft`.
- The connector's `inbound {event, bufferId}` → Hermes' `inbound_ack
  {bufferId}`, once its handler took the event. The event is `{text,
  message_id, source: {platform, chat_id, chat_type, chat_name,
  chat_topic?, user_id, user_name, profile, message_id}, media_urls?,
  media?, prompt_response?, context?, reply_to_message_id?, reply_to?}`.
  `source.profile` picks the profile of the multiplexed gateway. Hermes
  drops an event it has seen, by `(platform, chat_id, message_id)`, so a
  replay is harmless. `chat_topic` goes into the session's system prompt,
  once per session, as an untrusted label. `context` (`[{text,
  source?}]`) goes before the message as read-only channel context. A
  quote (`reply_to_message_id`, `reply_to: {text, author?, is_own}`) goes
  before the message as `[Replying to: "…"]`. No field changes how a
  session's unanswered tail is read: two user messages in a row are
  joined.
- A text that starts with `/` is a command from Hermes' registry. Idle,
  it runs as a turn. Busy, it is dispatched at once, never queued
  (`/steer`'s words land after the running turn's next tool call). No
  slash access is set, so the connector is the gate.
- Hermes' `outbound {requestId, action}` → the connector's
  `outbound_result {requestId, result}`, within 30 s. The ops the bridge
  advertises: `send {chat_id, content, reply_to, metadata}` (a reply when
  it answers a message, else tool progress, off in our image, or a
  notice); `edit {message_id, content}` (the
  whole text again); `delete`; `typing`; `react {message_id, emoji,
  remove}`; `draft {draft_id, content, final}` (a reply streaming; its
  final is a separate `send`, as is every segment Hermes ends at a tool
  boundary); `prompt {prompt_kind, prompt_id, content, options: [{id,
  label, style?}], timeout_s?}` (buttons); `send_media {media_kind,
  source_url, content, filename?}` (an upload to `/relay/media`, or a
  public URL); `get_chat_info`. Hermes also has `thread_create`,
  `thread_rename`, Slack's `task_card` and Discord's `follow_up`.
- A prompt's answer is an `inbound` whose event has `prompt_response:
  {prompt_id, option_id}`. Hermes resolves it at once, even mid-turn.
  The first answer wins.
- A clarify with choices is a `prompt` of `prompt_kind: "clarify"`
  (options `c0`… and `other`). Hermes' clarify takes free text only once
  "Other" is chosen; other text cancels it and runs as a message. A
  clarify waiting on its person never sees an interrupt; the word `skip`
  lets it go.
- The connector's `interrupt_inbound {session_key, chat_id}` is Stop, for
  the session key `agent:<profile>:relay:group:<chat_id>`. Stop is never
  sent as a `/stop` message: one sent while a session is busy stalls
  Hermes' reader.
- Hermes' `going_idle` → the connector's `going_idle_ack`.
- **A turn's only end is its reactions**: `react` `👀` on as it starts,
  off as it ends, then `✅` or `❌` (a cancelled turn gets neither). A
  message taken while the gateway starts is bracketed twice, the first
  time empty. Hermes ends no person's turn without saying something.
  Relay carries a question in words only as text (`❓ …`, or `✏️ Type
  your answer:` after "Other"), a notice only as a `send` answering
  nothing that starts with a glyph (`wire::NOTICES`), and a tool step not
  at all: our image's plugin observes it (above).

How the bridge uses it is `runtime/relay/mod.rs`'s header. Its choices:
the bridge serves `/relay`, `/relay/media` and `/relay/activity` on
`BRIDGE_RELAY_LISTEN`. A turn is an `inbound` group message, its chat id
`<chat fragment>/<agent fragment>`, so two agents in one chat are two
chats to Hermes. Its message id is the turn's id. A message's files are
downloaded first to `<BRIDGE_MEDIA_DIR>/<sha256>`, a scratch no save
keeps, and named after its words (name, type, path); no name from a
record becomes a path. A file Hermes sends by its provider's URL is
fetched by the bridge (`relay/fetch.rs`).

- **A routine with nothing to tell says nothing.** A model's bare
  `[SILENT]` gets Hermes' warning instead (`wire::UNEXPECTED_SILENCE`):
  Relay carries no `reply_expected`, so to Hermes every message needs a
  reply. For a routine's turn alone, the bridge drops that warning, whole
  and in those words, and the turn ends idle with no reply. This is done
  in the bridge, not in Hermes: a turn Hermes kept silent would end in an
  empty bracket, which the bridge cannot tell from the startup's.
