# The bridge

`images/bridge` (Rust, its own workspace with `images/hermes/boot`): the
one process a computer image runs between its agent runtime and the
fragment API (docs/computers.md). It owns nothing (lesson 1): the chat
channel owns what was said, the runtime owns the in-flight turn, the
Computer DO owns the computer. The bridge translates, and keeps only a
cursor per followed channel and the turns it admitted that have not
ended.

- `engine.rs`: the rules, as a pure state machine (records and runtime
  events in, posts and runtime commands out). docs/chat-records.md is
  what it writes.
- `driver.rs`: the restore gate, the state file, followers, posting
  lanes, the keepalive.
- `ready.rs`: which of the computer's agents to run, when the image says
  (`BRIDGE_AGENTS_FILE`): a computer's agents may change while it runs.
- `runtime/`: `relay` (Hermes' Relay connector) and `script` (a
  deterministic agent, the stub image's).
- `screen.rs`: each agent's screen (`?agent=<agent fragment>` on its
  sockets): a page, and an RFB proxy onto that agent's own display with
  Take over / Give back. Its input gate follows every message noVNC 1.7.0
  sends (the extended clipboard's negative length, the extended pointer
  event), and its control socket answers on an image with no display (the
  stub's) too. An agent the bridge does not run, or one the image names
  no screen for, is 404; no agent's name, 400.
- `screens.rs`: which display is each agent's, when the image says
  (`BRIDGE_SCREENS_FILE`), with its runtime's lease and activity file.
- `lease.rs`: who drives a screen, the agent or one person, as Hermes'
  Bot Desktop lease file has it (`lease.json` under its `lease.lock`
  flock, its epoch one on at every change): Take over is that lease, so
  the agent's tools refuse while a person holds it (`human_has_control`).
  A screen with no lease file keeps one of its own, by the same rules.

## A runtime

A runtime gets commands and sends events, each naming its turn
(`runtime/mod.rs`):

| Command | |
|---|---|
| `Start(TurnStart)` | a turn: agent, chat, asker and their name, text (`command`: a command of the runtime's menu, said as it is; below, "Commands"), attachments as local files, the message it quotes (`quote`: its text and whether it is the agent's own, read from the chat by the driver), whether it is a routine, and its `note` when the agent's turn before it in the chat was cut by a restart (below, "The turn after a cut one is told") |
| `Stop {turn}` | the asker pressed Stop |
| `Answer {turn, prompt, option?, words?, attached?}` | a prompt's answer, or its expiry (`None`), once; `words` for an option answered in words, typed on its card or sent as the owner's message in the chat while it waited (the message is that option's answer, no turn of its own); `attached`, that message's files, which the driver downloads and names in `words` (below, `Tell`) |
| `Forget {turn}` | the bridge ended it (it went quiet 15 minutes, or its agent left) |
| `Tell {turn, seq, by, text, attached?}` | the asker's next message while the turn asked them in words (`Asked`): its answer, handed to the running turn, never a turn of its own; only ever to a turn this life runs. Words, files (photos sent as the answer) or both: `attached`, its files, which the driver downloads as a turn's are (`BRIDGE_MEDIA_DIR`), then adds a line for each to `text` (its name, its type, and where it is on the computer: `runtime::with_files`) and takes off, as a runtime takes an answer in words alone (Hermes' clarify does) |
| `Aside {agent, fragment, turn?, id, by, text}` | a command said at once beside whatever runs in the agent's chat, no turn of its own (`/steer` into the running turn `turn`, `/btw`): below, "Commands" |

| Event | |
|---|---|
| `Connected(bool)` | the runtime can take turns now, or cannot: the bridge claims a turn only while it can (Relay: Hermes on its socket, greeted; `script`: from its start) |
| `Draft {text}` | the reply so far, shown live |
| `Reply {part, text}` | reply `part` (from 1) whole; posted at the next part, step, prompt, or end (at a step, `interim`: the turn works on after it, unless the step only updates memory) |
| `Attachment {part, file}` · `Retract {part}` | a file on a reply; a reply taken back |
| `Step {tool, args, ok, excerpt, category}` | a tool call, and what it does (docs/chat-records.md, `turn.step`); the model's words before it are a reply part of their own, sent before it |
| `Prompt {prompt, text, options, ttl?}` | a card; the turn waits, its computer kept awake until the card is answered (a press, or, for a card that takes words, its owner's message in the chat: `Answer`) or expires |
| `Asked` | the turn asked its asker something to answer in words (the question is a reply part before it): their next message to the agent in that chat is its answer (`Tell`); it waits, running, as long as a prompt's life, and the chat is told (`turn.asked`) |
| `End {outcome}` | `idle`, `stopped`, or `error` |
| `Say {agent, fragment, text}` | said with no turn running, or answering a command said beside one (`Aside`): a turn of its own |
| `Notice {agent, fragment, category, text}` | a notice, neither a step nor a reply: a `turn.notice` of the agent's turn running in that chat, or of the last this life ran there (none: not posted) |

A runtime also names its **menu** (`Runtime::menu`): the commands it
takes from its agents' owners, each with how the bridge carries it
(`MenuItem`, `How`; below, "Commands"). None by default.

Another runtime is another module here, or a bridge of its own.

## The routes it calls

Every request goes to `FRAGMENT_API` (`http://api.fragment.internal`)
over plain HTTP, with `x-fragment-agent: <agent fragment>` unless noted.
Bodies are JSON. `api.rs` has one method for each.

| Method and path | Body | Reads |
|---|---|---|
| `GET /api/computer` (no agent) | | `{computer, owner, image, agents: [{fragment, identity, name, owner}]}`; at start, then every minute, and within a second of a change to the ready file (below); `?simplex=1` when the image names a SimpleX core (below, "SimpleX") |
| `GET /api/computer/relay?host=&port=` (no agent), WebSocket | | the XFTP relay (docs/computers.md): one per connection a SimpleX core makes to a host and port the view's `relay` names, through the cores' proxy (`src/simplex/socks.rs`), its bytes piped both ways |
| `GET /api/computer/keepalive` (no agent), WebSocket | | held while a turn waits to run, runs, or waits on its card (below, "A card keeps its computer awake"); pinged every `BRIDGE_KEEPALIVE_PING_MS` (15 s), and opened again after twice that with nothing heard, since a deploy can cut the platform's side of it with no close reaching here (#557) |
| `GET /api/fragments?for=<owner>` | | `{fragments: [{name, kind, owned, title}]}`: its owner's, as the agent reaches them: its own chat with them found by its label (`api::bot_chat`), as a routine with no chat fires |
| `POST /api/fragments?for=<owner>` | `{name, template: "chat", title}` | the agent's SimpleX chat, made for its owner by the name its link chose, when its link is bound; 409, made already (then `GET /api/f/{chat}/members` says whether it is the agent's and its owner's: below, "SimpleX") |
| `GET /api/f/{agent}/file?path=agent.json` | | its home (`api::Home`), read as a routine with no chat fires (docs/api.md, "An agent's home"); none (404) is its own chat |
| `GET /api/f/{agent}/status` | | `{title}`: the agent's name (docs/api.md, "An agent's name and picture"), read every 10 s while its SimpleX core runs (below, "SimpleX") |
| `GET /api/fragments` | | `{fragments: [{name, role}]}`: what the agent follows, listed at start, on a `joined` of it (as the agent the engine runs, no read of the computer first), and every 5 minutes. A listing a read of which fails with no answer, a 429 or a 5xx is asked again after a jittered backoff (from 1 s, at most 30 s) until it answers: the list, or only the fragments whose channels did not read; what was found meanwhile is followed, so one fragment that fails keeps the agent from no other. One listing an agent at a time: a newer one takes the place of one under way |
| `GET /api/f/{f}/channels` | | `{channels: [{name, post, seq}]}`: a postable `chat` is a chat; the agent's own `tasks` |
| `GET /api/f/{f}/members` | | `{members: [{principal, kind, addedAt}]}`: the lead (first `kind: agent` by `addedAt`), and when this agent joined; read again after 30 s, and at once after a `joined` of one of the computer's agents to that fragment (so its lead never answers an `@mention` of an agent that just joined) |
| `GET /f/{f}/__people?id=…` | | `{profiles: {id: {kind, email?, name?, title?}}}`: what to call a writer: a person by their email (which its agent, a member, is shown), another agent by its name (`title`, its agent fragment's), else its label (`name`), read again with the chat's members (30 s), so a rename reaches it; none is "someone" |
| `GET /api/f/{f}/subscriptions` | | `{subscriptions: [{id, principal, channel, wake}]}` |
| `POST /api/f/{f}/subscriptions` | `{channel, wake: true}` | `{id, channel, wake}`; only when the list has none |
| `GET /api/f/{f}/channels/{c}?after=&limit=1000` | | `{records: [{channel, seq, at, principal, kind, body}], next}`: the catch-up, at most 20 pages; a turn's note, read back from its claim on `work` (and from `chat`'s tail) in pages of 100, at most 1000 records each; and the message a turn's message quotes (`reply_to`: `after=<seq − 1>&limit=1`, within 5 s or none) |
| `GET /f/{f}/__live`, WebSocket | `{type: "subscribe", channel, after}`, `{type: "ping"}` | `hello`, `record`, `subscribed {next, more}`; 4003/4004 end the follow |
| `POST /api/f/{f}/channels/{chat\|work}` | `{id, body}` | `{record, replayed}` (a claim's `record.seq` is where the turn's note is read back from); retried 10 times with jitter on a transport error, 429 or 5xx. A turn's claim (its `turn.start`) is answered back to the engine: posted or replayed, this life runs it; 409, another life claimed it; 403/404, the agent may not post there (it left the chat, or its owner holds it below editor), and the turn is dropped; anything else, no answer. Any other 409 is another life's post of the id, and a 403/404 is no longer the agent's to post in: dropped and logged |
| `PUT /api/f/{f}/channels/chat/draft` | `{turn, text \| null}` | at most 4 a second a turn; a 429 is ignored |
| `PUT /api/f/{f}/blobs/{sha256}` | the bytes, `content-type` the file's | a reply's files, before its record |
| `GET /api/f/{f}/blobs/{sha256}` | | a message's files (at most 25 MiB), checked against their hash |

The script runtime (the stub's) also calls, as its agent acting for its
owner, `GET|POST /api/f/{agent}/schedules?for=` and `DELETE
/api/f/{agent}/schedules/{id}?for=` on its own fragment (its `schedule`
command: docs/api.md, "Scheduled records"), as `fragment schedule` does
on a computer.

`hermes-boot` also calls, as each agent, on its own fragment:
`GET /api/f/{agent}/files`, `GET /api/f/{agent}/file?path=` (its
`SOUL.md`, `memories/`, `skills/`, `agent.json`), and
`POST /api/f/{agent}/files {files, message, key}` (at most 16 files and
180 KiB a commit; `key` is the batch's hash).

## Settings

`fragment-bridge run`:

| Variable | Default | |
|---|---|---|
| `FRAGMENT_API` | required | the fragment API |
| `RESTORE_PENDING` | | `1`: wait for `BRIDGE_RESTORED` first |
| `BRIDGE_RESTORED` | `/run/computer/restored` | |
| `BRIDGE_HOLD` | `/run/computer/hold` | while this file exists the bridge claims no turn (the platform's hold before every save; looked at before each claim's every try) |
| `BRIDGE_HELD` | `/run/computer/held` | the bridge's answer to the hold: it writes this file while the hold exists and no claim's try is in flight, and removes it otherwise (looked at every 100 ms). An image with more to quiet than the bridge names another file and answers the platform itself |
| `BRIDGE_HELD_LEAVE_OUT` | none | what the save may leave out, written into the answer one per line (whitespace between here): gitignore patterns relative to `/data`, of letters, digits and `._-/*?[]`, at most 16 (docs/computers.md, "The hold"). The stub names `*.scratch` |
| `BRIDGE_RUNTIME` | `relay` | `relay` or `script` |
| `BRIDGE_STATE_DIR` | `/data/bridge` | its state; made as it starts (past the restore gate), so a first start has a `/data` to save |
| `BRIDGE_AGENTS_FILE` | | the agents the image has made ready (`src/ready.rs`): `{"agents": [fragment]}`, written whole and renamed into place. Set, the bridge runs only those of `GET /api/computer`'s, in the platform's order, and reads the computer again within a second of the file's change; missing, no agent is ready; one that does not read keeps the set before it. Unset, every agent the platform lists (the stub). Our Hermes image's is `/var/lib/fragment-run/agents.json`, written once each new agent's profile is whole (docs/computers.md) |
| `BRIDGE_MEDIA_DIR` | `/tmp/bridge-media` | attachments, scratch |
| `BRIDGE_MEDIA_LOCAL` | | `allow`: a file Hermes sends by URL may come over plain `http`, from a local address (a test's fake server: tests/docker.rs). Unset in every image |
| `BRIDGE_PROMPT_TTL_MS` | 3 600 000 | a card's life unless the runtime says |
| `BRIDGE_TURN_IDLE_MS` | 900 000 | a running turn this quiet ends as an error |
| `BRIDGE_KEEPALIVE_PING_MS` | 15 000 | how often the keepalive pings; silent twice that, it is opened again (#557) |
| `BRIDGE_RELAY_LISTEN` | `127.0.0.1:8650` | where Hermes dials |
| `BRIDGE_RELAY_SECRET_FILE` | required for `relay` | the per-boot secret (32+ characters) |
| `GATEWAY_RELAY_ID` | `fragment-computer` | the gateway id both sides name |
| `BRIDGE_SCRIPT_PACE_MS` | 40 | the scripted agent's draft pace |
| `BRIDGE_SCREEN_LISTEN` | | `0.0.0.0:6080`: serve the screens. Their sockets wait (at most 30 s) for the agents, told past the restore gate, so no lease under `/data` is touched before the restore |
| `BRIDGE_SCREEN_DIR` | `/opt/fragment/screen` | its page |
| `BRIDGE_SCREENS_FILE` | | each agent's screen (`screens.rs`): `{"screens": [{agent, rfb, lease?, activity?}]}`, `rfb` as `unix:<path>` or `tcp:<host:port>`, written whole and renamed into place, read again when it changes. Unset, no agent has a display (the stub); set, an agent it does not name has no screen. While someone watches a screen its `activity` file's time is set every 10 s (`ACTIVITY_EVERY_MS`), so the image's idle stop leaves a watched desktop up. Our Hermes image's is `/var/lib/fragment-run/screens.json` (docs/computers.md) |
| `BRIDGE_SCREEN_START` | | the command that starts an agent's display, run with the agent's fragment as its last argument by a viewer that finds it down: at once when it answered since the last start (it stopped or restarted under its viewers, whose streams end with it), else at most once a minute while it stays down |
| `BRIDGE_SIMPLEX_BIN` | | SimpleX's core, `simplex-chat` (below, "SimpleX"): set, each linked agent's core runs while the computer is always on; unset (the stub), nothing of SimpleX runs, and `/simplex/…` is 404. Our Hermes image's is `/opt/fragment/bin/simplex-chat`: root's, 0700, on no PATH of its agents' |
| `BRIDGE_SIMPLEX_DIR` | `/data/simplex` | each agent's SimpleX state, `<dir>/<agent fragment>/`; made 0700 at every start, the bridge's alone |
| `BRIDGE_SIMPLEX_SMP`, `BRIDGE_SIMPLEX_XFTP` | | servers its cores use (a test's lab); unset, the core's own presets (SimpleX Chat's and Flux's) |

## State, and what it never does twice

`<state>/state.json`, written whole (a temporary file, synced, renamed)
after every step that changed it and before that step's effects: the
cursors (`agent|fragment|channel` → seq), the turns not yet over
(queued, running, waiting, or ended and owing their last records), and
each chat's budget of agent turns ("Agents asking each other", below). It
is a cache. The authority on which turns have started is the chat's `work`
channel, which never goes back in time, so `/data` restored from any
earlier save, or lost, costs reads and runs nothing twice
(docs/explorations/pi-durable.md, P1).

- **One life per turn.** Each bridge process is a life, with 128 random
  bits of its own, never written to `/data`. A turn is run only by the
  life whose claim, its `turn.start` naming the life, the platform
  answered as its own (docs/chat-records.md); a 409 is another life's,
  and the turn is ended as lost; a 403/404 (the agent may not post on the
  chat's `work`) drops the turn, run and recorded nowhere, and is not
  asked again; no answer runs nothing, and the turn is claimed again at
  the next tick. A cursor a rollback sent back reads a record again, and
  its claim is the 409.
- **At a start**, every turn the state holds that is not queued belongs
  to an earlier life and is ended (`error: lost when the computer
  restarted`, its cards `expired`), never handed again: this is the
  at-most-once choice, so a turn whose life ended before it did is lost,
  and said so. Queued turns stay queued and are claimed like any other.
- **A claim waits for the runtime and the hold**: a turn is claimed only
  while the runtime says it can take one (`Connected`), and not while
  `BRIDGE_HOLD` exists (the platform's hold before a save; the file is
  looked at before every try of every claim). Until then it waits,
  unclaimed, and any later life may run it. The bridge answers the hold
  (`BRIDGE_HELD`) only once no try is in flight (from its look at the
  hold to its answer), so a save never misses a claim that lands after
  it; a turn already claimed runs on, and its keepalive is what cancels an
  idle sleep's hold (docs/computers.md).
- **Every turn has both records**: a refusal, and a Stop of a turn that
  waited, post its `turn.start` and then its `turn.end`.
- **An answer in words goes only to its turn's life.** A turn that asks
  its asker something in words (`Asked`) is a running turn of its life,
  and their next message to the agent in that chat is its answer
  (`Tell`), starting no turn: its words, its files, or both. A turn
  waiting on a card that takes words (a clarify's "Other") takes its
  owner's next message to the agent there the same way, as that option
  answered in those words (`Answer`), the card closed with them (Paul on
  production, 2026-10-22: a reply typed in the message box stayed
  queued, the card open, for up to its hour). Someone else's message
  waits its turn, as does any while the card takes no words (an
  approval's Allow and Deny: a press alone answers it, and a yes is no
  guess to make from a sentence). A restart ends it as lost, as it ends every
  running turn, so a later life tells it nothing: the message, read
  there, is a turn of its own, claimed and run once. So is one a rollback
  sends a cursor back before, though an earlier life told it (a Tell
  writes nothing on `work`). A Stop answers the question too (Relay says
  `skip`), so the asker's next message queues behind the stopping turn.
- What the runtime says unasked is a turn of the life's counter and the
  life, so a counter a rollback sent back collides with nothing.

A state that contradicts itself (a turn past its cursor, an id not its
cause's) is refused at start, never repaired. A state of another format
(an older bridge's) is set aside for a fresh one: the journal says which
turns ran.

- **A turn leaves the state only once its last records are answered.**
  The state is written before a step's effects, so a turn that has ended
  there would otherwise have its `turn.end` only in its fragment's lane,
  and a crash then would leave it open for good (its start, and no end),
  or leave a refused or queued-and-stopped turn with no record at all.
  So an ended turn is kept (`ended`), holding what it owes `work`: its
  `turn.end`, and for a turn that never ran its `turn.start` too, with
  their bodies. It is let go once the lane is done with each: answered
  (appended, a replay, or refused: a 409 is another life's end, a
  403/404 a chat it may not post in), or given up by the lane's rule for
  every post, `POST_TRIES_MAX` (10) tries with a jittered backoff on a
  transport error, 429 or 5xx. A life that ends first leaves them to the
  next, which posts every one again at its start under the same id and
  body: a replay when it had landed. This closes the gap the simulation
  found (`any_history_of_crashes_and_rollbacks_keeps_the_invariants`
  checks I2 with no case let off; `a_crash_that_drops_a_turns_end_leaves_
  nothing_open` and `an_end_a_crash_kept_from_the_platform_is_posted_by_
  the_next_life` each way it happened). A record that keeps failing holds
  its turn for one life's tries, and each later life tries it again once.
  Not kept: the turn's last reply, and its cards' closings, posted in the
  same step; a crash before they land loses them. A turn refused past
  `TURNS_OPEN_MAX` is not kept either.

Every bound is a const in `limits.rs`, with its reason. SIGTERM: gone
within 3 s.

## Agents asking each other

A person's agents hand work to each other in a chat they share, by
`@name` in a reply (the bridge stamps its `to`), or by a message one posts
itself naming the other in `to` (`fragment ask`, cli/src/ask.rs; in our
Hermes image, a bot's `message_agent` too, which the image posts into the
other's own chat that way: docs/computers.md, "Bot Mode in our Hermes
image"). The bridge that answers decides whether it is a turn, and how
deep (docs/chat-records.md, "An agent's reply"):

- **The hop is counted here, not read.** A record by an agent of this
  computer is one hop past the turn that agent is in: its turn running in
  that chat, or the one there the record's `turn` names when that ended
  within `ENDED_HOPS_MS` (remembered, never written: a reply read just
  after its turn was let go); and its deepest turn running anywhere else,
  the deeper of the two; in none at all, `HOPS_MAX` (answered, handing on
  nothing). A CLI post names no turn, so a turn just ended does not count
  for it (`an_ended_turn_counts_only_for_the_reply_that_names_it`). A record's `hop` only raises it. So a post made
  around the bridge (the CLI, the API, a script the agent left running)
  resets nothing: `a_hand_off_loop_stops_at_the_cap` runs the same
  A, B, A, B loop with each hand-off posted by its turn as the CLI does (no
  `hop`, no `turn`) and stops at the same place, and `a_scripted_hand_off_loop_stops_at_the_cap`
  (tests/bridge.rs) runs it with the scripted runtime, then an agent's
  CLI-shaped post, answered once. What is remembered of ended turns is
  never written to `/data`: a reply the next life reads first is the last
  hop (`a_reply_read_by_the_next_life_resets_nothing`). Another
  computer's agent (another person's, in a shared chat) is held to the hop
  it claims, at least 1, and by the budget.
- **A chat's budget.** Turns its agents start of each other are kept per
  chat in the state (`agent_turns`: the causing records' `at`s, the
  platform's clock, so every life counts alike), at most
  `AGENT_TURNS_PER_CHAT_MAX` in `AGENT_TURNS_WINDOW_MS`; past it a
  hand-off's turn is refused with both its records, its end saying why
  (`a_chats_agents_have_a_budget`: valid, past it, a person never
  counted, replay, restart, another chat's own, the window sliding). A
  state from before the budget loads with none.
- **`tasks` is the agent's own.** A routine or `joined` starts anything
  only from the agent fragment's own key (not an identity: its cron, the
  platform) or its owner; another agent acting for the owner, who may post
  there, is passed over (`tasks_hear_only_the_owner_and_the_fragment`).
- **A routine that names no chat runs at home**: as it is read, its
  follower reads the agent's home (its `agent.json`; for SimpleX, the chat its
  link names while bound; else its own chat, from its owner's list and
  its own: a chat it is not in is no home)
  and names that chat in it before the engine sees it (`driver::home_of`,
  `records::homed`), so a home changed later moves the next fire. A home
  that does not read ends the follow, which reads the routine again from
  the engine's cursor after a wait: no fire is lost to a failed read, and
  one past the hour a missed routine is skipped after is not read for. A
  SimpleX chat its link has named and not made yet (not in the agent's
  own list: its making refused for now, or a restart between its naming
  and its making) is waited for the same way, up to
  `limits::HOME_CHAT_WAIT_MS` (10 minutes) from the routine's firing, and
  then it runs in the agent's own chat: never dropped (`api::home_chat`;
  `a_routine_waits_for_its_simplex_chat`: past the wait, waiting across a
  restart, run once made, nothing twice after a restart). An
  agent with no chat with its owner at all runs it nowhere
  (`routine.homeless`). A bridge before homes passes such a routine over
  (`a_routine_with_no_chat_runs_at_home`, tests/bridge.rs: valid, a home
  changed, a failed read, no home, a restart).

Why here: the platform holds no chat record (docs/cloudflare-v1.md, the
rule), and the bridge is the one place that knows which turn an agent is
in. A person's agents all run on one computer (decision 13), so every
hand-off between them meets this count.

## Commands

An agent's owner commands its runtime from the chat (docs/chat-records.md,
"A command"): only the runtime's menu (`Runtime::menu`), only from the
agent's owner (`GET /api/computer`'s `owner` of the agent), for the agents
the record's `to` names, else the lead. Anyone else's, and one off the
menu, is passed over (`command.ignored`): nothing else reaches a runtime
as a command, and a message's leading `/` never does (Relay: a zero-width
space before it).

- **How each is carried** is its menu item's (`How`, `engine.rs`
  `command`): `Turn`, a turn of its own, admitted as a message is (its
  place in the chat's queue, both its records, Stop), its text
  `/<name> <args>` handed as it is (`TurnStart::command`, kept in the
  state: one waiting its turn is a command in the next life too); `Stop`,
  the agent's running turn in the chat stopped whoever asked it (its open
  cards closed `stopped`) and its waiting ones ended unrun, each with
  both its records; `Restart`, the same, then a turn of its own, which
  runs once the stopped turn ends; `Message`, its words a message; `Steer`,
  its words said into the agent's running turn in the chat at once
  (`Command::Aside`, no turn and no record of the bridge's), or a message
  when none runs; `Aside`, said beside whatever runs, at once. A menu
  item's `refuse` names words of its args that refuse it: its turn's two
  records, the end saying why (`a_refused_command_ends_saying_why`).
- **What answers an aside is a message of its own** (`Event::Say`), never
  the running turn's reply or step: the runtime tells the bridge which
  sends answer it.
- **The menu in each chat.** The agent's first turn handed in a chat in
  a life posts the menu on `work` (`commands`, id `cm:<life>`) before the
  runtime hears of the turn, so the page offers it to the owner
  (`records::commands`). Kept per life in memory, never in `/data`.
- Proven in the engine (`a_command_from_the_owner_is_a_turn_of_its_own`:
  valid, replay, restart; `a_command_from_anyone_else_or_off_the_menu_
  starts_nothing`; `stop_stops_the_running_turn_and_ends_the_waiting`;
  `new_stops_then_runs_its_own_turn`; `steer_queue_and_btw`), through the
  bridge with the scripted agent (tests/bridge.rs `commands_and_quotes`)
  and the scripted Hermes (tests/relay.rs `commands_said_as_they_are`),
  and with the real Hermes (tests/docker.rs
  `commands_and_quotes_reach_hermes`).

The scripted agent's menu (`script::MENU`) has one command of each way,
so the platform's lanes run each with no agent runtime: `usage` and
`model` (`ran /<name> <args>`), `new`, `stop`, `steer` (into a `steer-me`
turn, which waits for one), `queue`, `btw` (`aside: /btw …`).

## A card keeps its computer awake

A turn waiting on its card holds the keepalive until the card is
answered or expires, at most the card's life (`BRIDGE_PROMPT_TTL_MS`, an
hour; docs/durable-computers.md, P6 for now). So a card expires with its
runtime there: Hermes' wait on it times out with it (the image sets
`agent.clarify_timeout` and `approvals.timeout` to the card's life), its
model is told no one answered, the turn ends, and the chat's next message
is answered.

It used to be let go (decision 42 as first written), so an idle computer
slept 20 minutes into an unanswered card, and the next life ended its turn
as lost. Hermes, though, had kept the cut turn's message as its session's
last, unanswered, and folds the next message into it (two user messages
in a row are one): the model was given `[paul] do the risky thing\n\n[paul]
good morning`, and asked the cut command's approval again. Every message
after a missed card was met by the card again, and the computer slept
under each one (Paul on p5, 2026-10-05: "I missed the 1hr window and now
it's not responding to chats"; `an_expired_card_ends_its_turn` in
tests/docker.rs, and its scripted twin in tests/relay.rs). A restart for
any other reason while a card is open (an owner's sleep, a crash, a
deploy) still cuts the turn, which ends as lost; the next message is now
a turn of its own, told what was cut (below), never folded into the cut
request. The card's promise outliving a restart is P6 of
docs/explorations/pi-durable.md, and the debt ledger.

## The turn after a cut one is told

A turn a restart cut ends as lost (one life per turn: "State", above),
and is never run again. What it did before the cut may have effects (an
email sent, a file half written), and its runtime may still hold its
request: Hermes keeps a turn's message as its session's last until the
turn answers, and joins the next message to it (two user messages in a
row are one), so its model was handed `[paul] do the risky thing\n\n[paul]
good morning` and did the risky thing again (F10 of
docs/explorations/pi-durable.md; P5 of docs/durable-computers.md). Two
parts close it:

- **The note (every runtime).** The first turn an agent runs in a chat
  after one of its turns there ended as lost carries a note
  (`TurnStart::note`, `src/note.rs`): its first line says the turn before
  was cut short by a restart and to check what it already did before
  doing any of it again (the message below is new); then what that turn
  was asked (its cause's text), the steps `work` recorded of it (tool,
  arguments, ok or failed; at most 8, the rest counted), its cards and how
  they closed, and the replies it had said (the last two). At most 4 KiB.
  It is built from the chat's journal alone: the driver reads `work` back
  from the turn's claim (the claim's answer names its seq) to the agent's
  turn before it there, a refusal's passed over, and, when that turn ended
  `lost when the computer restarted`, its cause and its replies (`chat`,
  back from its tail to that turn's cause or start). So it is the same in
  any life and after any rollback of `/data`, and said once: at the turn
  after, the turn before is this one. A journal that does not answer
  within 5 s, or an agent's turn before further back than 1000 records,
  gives no note: a turn is never held back longer for one. Relay hands it as the inbound's
  read-only `context` (below); the scripted agent echoes it after its
  reply (`(told: …)`).
- **The cut turn closed in Hermes' session (our image).** Before the
  gateway starts, `hermes-boot` appends Hermes' own failed-turn boundary
  (the assistant row Hermes writes itself when a turn ends without an
  answer, `display_kind` `failed_turn`) to each relay session whose last
  message is unanswered (a user row, a tool call, or a tool result),
  through Hermes' session code (`hermes::CLOSE_CUT_TURNS`;
  docs/computers.md). The next message is then a user message of its own.

Proven with the real Hermes in `a_turn_cut_by_a_restart_is_closed_and_told`
(tests/docker.rs): an owner's sleep under a card, then "good morning": no
second call of the cut command, no card again, an answer, and the model's
request ends with the note and the message after the boundary. Whether a
real model, so told, checks before it redoes is a hosted run's to see.

## What a rollback forgot

A computer that went back to an older save (a crash, an unsaved stop, a
restart whose save failed: docs/computers.md, "What its owner is told")
restores a runtime that remembers nothing after that save, while the chat
keeps every turn since. The bridge's state went back with it, so it reads
those turns' records again; each claim answers 409 (the life that ran them
claimed them: "State", above), so none runs twice. Those 409s are what the
agent forgot, exactly: the engine keeps, per agent and chat and in this
life only, the turns another life claimed (the newest `NOTE_FORGOTTEN_MAX`,
4, and how many more), and the agent's next turn in that chat takes them
(`TurnStart::forgotten`). Its note then says, from the chat's journal
alone (`note::forgotten`: each turn's start on `work`, its end, and on
`chat` its cause and its replies), that its memory of the chat is behind
because the computer went back to an earlier save, to check before doing
any of it again, and what each was asked and replied, after the cut
turn's note when there is one (a turn a restart cut is that note's, never
told twice), the two together at most 8 KiB. Said once in a life; a life that restores the same old
`/data` again reads the 409s again, and is told again. Proven in
`turns_another_life_ran_are_told_once_to_the_next_turn` (the engine),
`forgotten_turns_are_told_from_the_journal` (note.rs), and on the platform
by the `computers` lane's recovery checks, where the stub echoes it.

## SimpleX

An agent's owner may link the agent to their SimpleX app (decision 32;
`src/simplex/`; what it adds to the platform, entry by entry:
docs/explorations/simplex-complexity.md). The bridge runs SimpleX's own
core, `simplex-chat`, one per linked agent, as a child process, driven
over its bot API (`-p`, loopback only) by one task, so nothing else ever
drives it.

- **SimpleX is a chat of its own** (Paul, 2026-10-26): the agent's
  SimpleX chat, its owner's chat labelled `<label>-simplex` and titled
  SimpleX (docs/api.md, "An agent's home"). Its name in full is chosen
  once, as the link is first bound, and written to `link.json` before
  anything makes it (`Saved::chat`: write-ahead); the agent's poster then
  makes it for them by that name (`POST /api/fragments?for= {name}`, the
  agent its maker and an editor, told so as a `joined`), or, the name
  taken (409), finds it made when the agent is in it and its owner owns
  it, from its members, never from a list that may lag its making
  (`run::ensure_chat`). So every life makes that one chat, once, whatever
  restarts, and it shows among their chats before anything is said; a
  name someone else holds is chosen again. It mirrors their app both ways
  while linked:
  everything their linked contact sends is posted there, a relayed record
  as before; everything the agent says there goes to their app once
  posted, whatever started its turn (a SimpleX message, words typed in
  that chat on the web, a routine, a hand-off, something it says with no
  turn): its replies but the interim ones, with their files (below), and a card's
  question with its choices (`driver::for_simplex`). The chat is the
  engine's to know (`Input::Relaying`: each linked agent with its chat's
  name, read from `link.json` as the bridge starts, before any record, and
  named before it is made: so a post there is known to be the phone's
  from the first; `Engine::mirror`), so nothing of it is kept per turn and
  `state.json` holds nothing of SimpleX. An unlink stops the mirror and
  keeps the chat, its history and its name; a link again is that chat. A
  post refused there (the chat deleted meanwhile) has it made again by its
  name before the next. Until 2026-10-26 its messages went to the
  agent's own chat with its owner, and only a turn a SimpleX message
  started answered there (`via`, a turn's mark): gone, with the mark.
- **The agent is told in that chat alone**, as the chat's topic, on every
  inbound there (`TurnStart::simplex`, `Command::Aside`'s; relay
  `wire.rs`, `SIMPLEX_TOPIC`): "Your owner's SimpleX: what you say here
  reaches their phone." Hermes renders a topic in that chat's session
  context, in its system prompt, pinned per session and rendered again
  only when it changes: a few tokens, once, in that session. Not beside
  each message (a voice memo's transcript, which Hermes puts before the
  message and its context, would no longer read as the message's), and
  never as an instruction: Hermes reads a topic as a label. Where its
  owner's home is, it is told beside its name (docs/computers.md).

- **Its name in the owner's app is the agent's name** (its title, docs/
  api.md, "An agent's name and picture"; `run::display_name`), and follows
  a rename (Paul, 2026-10-26: his app showed the name his agent had when
  its bridge started, which the bridge read once, and the core's profile,
  made at its first start, was never named again). The bridge reads the
  title from the agent's status every 10 s while its core runs
  (`TITLE_EVERY_MS`, as our Hermes image does), and asks the core to show
  it (`/p '<name>'`, `wire::Cmd::Profile`) once each time it comes up and
  once a rename: the core keeps the rest of the profile (a bot's), sends
  the name to the linked contact, and answers `userProfileNoChange` for the
  name it holds, so asking again changes nothing. `--create-bot-display-
  name` names only the profile a first start makes, which is the agent's
  label: the core makes no profile of a name it refuses, and then does not
  start. A name is what the core keeps of the title, so it takes it as it
  is: `wire::valid_name` is v7.0.3's `mkValidName` ported char for char
  (by Unicode's general categories, `unicode-properties` pinned at
  Unicode 15.0, the core's own: a char of 15.1 or later is none to it,
  so dropped; never
  `.,;/\#@'"`~`, a combining mark only after a letter, at most 50
  chars), held to the core's own tests (its tests/ValidNames.hs) and, in
  the lab, to the real core with 200 seeded titles of every kind of char
  (Alex's review of #628). A title read that fails changes nothing; no
  title, or none of it kept, is the label. The greeting says the name
  too.
- **SMP on port 5223, never 443** (`--smp-web-port-servers off`,
  `simplex/run.rs` `core_args`). A SimpleX server on 443 takes its
  protocol only from a TLS hello that names no server (SNI), and
  Cloudflare Containers reset every such hello on 443: a core on a
  computer reaches SMP servers on 5223, and XFTP's (files: a voice note, a
  photo), on 443 alone, only through the platform's XFTP relay.
- **The cores' way out** (`src/simplex/socks.rs`): a SOCKS5 proxy on
  loopback, which every core connects through (`--socks-proxy`, with
  `--host-mode public`: hosts by their names, never their onion
  addresses). A connection to a host and port the computer's view's
  `relay` names (SimpleX's preset XFTP servers, on 443) goes through the
  relay (`GET /api/computer/relay`), its bytes untouched; anything else
  (SMP, on 5223) straight out of the computer. A platform with no relay
  offers none, and everything goes straight out. At most 64 sessions at
  once. It logs each relay's host, bytes and time, never what is carried.
- **A file not received within 3 minutes** (`FILE_WAIT_MS`: a server
  outside the relay's list, or one that does not answer) is given up on:
  cancelled at the core, and its message posted with a note that it did
  not arrive.
- **A reply's files go to the owner's app too**, after its text, each as
  SimpleX's apps show it (`run.rs`, `Handle::reply`: the one way anything
  goes out over SimpleX). Audio whose length reads goes as a voice
  message of that length (MP3, as Hermes speaks; MPEG-4, as the apps
  record: `audio.rs`), a JPEG, PNG or GIF as an image (with the preview
  SimpleX's own CLI sends), anything else as a file. Each is copied first
  to the link's `out/<folder>/<its name>`, the core uploads it over XFTP
  (through the relay, on a computer), and the copy goes once the core
  says it is sent (`sndFileCompleteXFTP`) or failed (`sndFileError`), or
  after `SEND_WAIT_MS` (5 minutes: cancelled at the core). At most 25 MiB
  a file (the chat's own bound: padded into XFTP's chunks, one relay's
  worth) and `SENDING_MAX` (8) under way. A file past either, empty or
  unread, or one the link cannot send now (unlinked, its core down, its
  ratchet out of step) stays in the chat alone, and the text says so:
  "(I sent a voice message, in our chat.)"; one that fails later is told
  as "(A voice message I sent didn't arrive here: it's in our chat.)".
- **Only where the image names the core** (`BRIDGE_SIMPLEX_BIN`), which
  the bridge's reads of its computer say (`GET /api/computer?simplex=1`),
  so its owner's Settings offer SimpleX only on an image that does it (the
  view's `simplex`; an image whose reads never said so shows nothing of
  it), and
  **only while the computer is always on** (its view's `alwaysOn`: the
  always-on seat; docs/billing.md). A computer that sleeps would hear
  nothing, so it runs no core at all, and its link page says why. A seat
  that lapses turns always-on off: the cores stop, and the computer sleeps
  when idle, its links kept for when it is on again.
- **The link page**, `GET /simplex/?agent=` on the screen port (in the
  bridge, `src/simplex/page.html`, so every image serves the same one),
  is what the shell opens from Settings' SimpleX, in a pane through a port
  ticket as a screen is, telling it the agent's name (`{fragment:
  "simplex", agent, name}`). It shows the one-time invitation as a QR code
  the bridge draws (`GET /simplex/qr.svg?agent=&s=`: the server's short link
  when it made one, which SimpleX's apps open, else the full link), with
  Open in SimpleX and Copy link; then whom it is linked to, and Unlink. Its
  words are for someone who has never heard of SimpleX, and it says
  plainly: encrypted between your SimpleX app and your computer.
- **Only its owner links or unlinks it, from their Settings** (Paul's
  design of 2026-10-23; docs/api.md, "An agent's doors"; `src/simplex/
  door.rs`). The agent's tools reach the screen port too, so the page's
  Link and Unlink ask the platform's page framing it for a ticket
  (`{fragment: "simplex", agent, want}`), which asks the platform for the
  agent's owner alone and hands back `{fragment: "simplex", agent, for,
  ticket}`; the page posts it as the body of `POST /simplex/link?agent=`
  or `/simplex/unlink?agent=`. The bridge takes one only signed by the
  agent's key (its view's `key`), for this computer and that agent, within
  its ten minutes, once (the nonces it took kept in `link.json` until
  they would have run out); a link's only while unlinked (else 409). With
  none, or with any other, it is 403 and nothing moves.
- **A link's door key**: before any invitation exists, the bridge makes a
  key for the link (`door.key` in the agent's directory: 0600, the
  directory 0700, in no process's environment) and redeems the ticket with
  its public half at the platform (`POST /api/computer/door`), which
  takes only the link ticket the owner made last, once. Every message the
  link carries to the agent's chat is a relayed record that key signs
  (`Key::seal`, at each try), and the platform takes no relayed record
  without its door's signature, so nothing the agent's own key posts
  speaks for its owner. The unlink's ticket closes the door at the
  platform as it is made; the bridge forgets the key when the link goes
  (an unlink, or the contact's own delete). A link with no key (one that
  went back in time to before its key) tells the owner's app to link
  again rather than carry what the platform would refuse.
- **The invitation is shown to the page whose ticket made it** (its
  nonce, `s`): `state` and `qr.svg` give anyone else no link (`hidden`),
  and that page's ticket's nonce is held in memory only, so after a
  restart, or from another window, the page asks for the code again with
  a new link ticket (the same invitation).
- **Linking** is a one-time invitation the core makes once its door's
  key is registered; the contact that connects through it is the owner's,
  and the spent link is refused to anyone after (the SMP server's
  `AUTH`). Any other contact is deleted, and nothing of theirs is
  carried. Linked, the core greets the owner's app. Unlinking lets the
  contact go (told in their app); their own delete in their app lets it go
  here. `GET /simplex/state?agent=` says where it is (`off`, `asking`,
  `inviting` with its link for the page that asked, `linked` with their
  name), whether it runs, and why not.
- **Its state** is `<dir>/<agent>/`: the core's two databases, its files
  and temp folders (one device: a received file is renamed from one into
  the other), `door.key`, `out/` (the copies of the files it sends), and
  `link.json` (the link, the agent it is of, the start it last ran in, the
  tickets it took, the files it receives and sends: the core resumes both
  after a restart, and a folder in `out/` it names no send of goes as the
  bridge starts), written whole before what it causes. One of another
  agent (a fragment's name made again) or that does not read is set
  aside, renamed, never guessed.
- **The hold stops every core** before the bridge answers it (`BRIDGE_HELD`
  waits for them), and none starts while it lasts: a save takes the cores'
  databases whole, as they are (`/_start` after `/_stop` crashes v7.0.3, so
  the process stops: SIGTERM, then SIGKILL past 3 s). Our Hermes image's
  copy of the databases under a hold passes `/data/simplex` over (held.rs,
  `SIMPLEX`). An awake save starts them again as the hold goes.
- **A start that went back in time** (the view's `restored.rollback`, for a
  link last run in an earlier start) forces a ratchet sync before anything
  is sent, holds replies through it, and tells the owner's app a message
  may have been lost: the core cannot see a rollback itself
  (docs/explorations/simplex-connector.md, "The lab"). Every start reads
  back the newest items, and forwards are by the sender's message id, so
  a core killed mid-burst loses nothing it committed.
- **A link's change asks for a save**: the keepalive held for 3 s, so the
  platform saves as that ends rather than after the next turn.
- **A core that stops on its own** is started again (2 s, doubled); three
  stops in 10 minutes and it waits for its owner to link again.
- **What a core prints is not logged**: it echoes the commands it is sent,
  message text among them.
- **The agent is kept out.** Every agent's system prompt says Fragment
  carries its owner's SimpleX, and never to install, run or set up
  SimpleX (our Hermes image's relay hint, hermes-boot's `SIMPLEX`, a line;
  which chat is the SimpleX one is that chat's topic, above). The core and its state
  are the bridge's: our Hermes image keeps `simplex-chat` root's, 0700 and
  on no PATH, and the bridge makes `BRIDGE_SIMPLEX_DIR` 0700 at every
  start, so the agent's own user (Hermes runs as `hermes`, the bridge as
  root) reads no core's keys or databases and no door's key. Its
  passwordless sudo still reaches root (docs/computers.md, "Root in our
  Hermes image"), and a core's bot API listens on loopback, which any
  user of the computer reaches.

Proven by the Docker lane's `simplex_on_the_hermes_image` (the real image,
a lab of a local SMP and XFTP server and the person's simplex-chat on a
network it joins): linked from the screen port by its owner's ticket
alone (none is 403), the invitation hidden from any other page, its door's
key registered with the ticket, by the name it was given after its bridge
started, and renamed while linked (the person's app follows within 10 s),
its SimpleX chat made for its owner; a
message posted there, a turn of Hermes' asked as its owner, its session
told the chat is their phone, answered on SimpleX (1.2 s); words typed in
that chat on the web, answered on SimpleX too; a routine with no chat, its
home that chat, answered on SimpleX; a reply in its own chat, not; a voice note
transcribed through the route and answered (0.4 s); a hold that stops the
core first, its copies naming none of the core's databases; a restart; and
a wake from a save older than the life before it, healed, the person told
(1.0 s after the gate). By hand, `tests/simplex.rs` holds the rest against
the real core: the link's rules, voice both ways, the profile's name (a
start keeps the profile its first made; `/p` renames it, idempotent;
every name `display_name` makes taken as it is), the lifecycle (a forged
ticket refused, the door key's file 0600 and in no environment, the unlink
closing the door at the fake platform), messages through the chat, and the
page's code, which another page cannot fetch. The platform's half (the
ticket's signature, its one redeeming, refusing a relayed record the
agent's own key or any other signs, and the unlink closing the door) is
the `computers` lane's door checks, through the stub's `door` commands.

## Hermes' Relay, as the bridge speaks it

Hermes v0.21.7 (tag `v0.21.7`, 2026-10-12; read first from v0.21.5,
whose wire it keeps; `gateway/relay/`), contract version 1. The bridge serves
`ws://127.0.0.1:8650/relay` and `/relay/media` and checks Hermes'
token; the descriptor names platform `relay` with draft streaming and
the ops `send, edit, delete, typing, react, draft, prompt, send_media,
get_chat_info`.

- Each turn is an `inbound` group message routed to the agent's profile
  (`source.profile`), its chat id `<chat fragment>/<agent fragment>` so
  two agents in one chat are two chats to Hermes, its message id the
  turn id. Claimed only while Hermes is on its socket and has said
  `hello` (the bridge starts just before Hermes' gateway, so the message
  that woke the computer waits for it, unclaimed); once handed, kept until
  Hermes acks it and handed again on each dial, within that life. A
  turn's note (above, "The turn after a cut one is told") is the
  inbound's `context`, one item with no source, never its text: Hermes
  renders it before the message, reference only and never as the person's
  words:
  `[Recent channel messages]\n<note>\n\n[New message]\n[paul] good morning`.
  In its owner's SimpleX chat every inbound's source carries
  `chat_topic` (`wire::SIMPLEX_TOPIC`), which Hermes renders in that
  session's context as `**Channel Topic:**` (above, "SimpleX").
- A reply streams as `draft` frames (the chat's draft), and arrives as a
  `send` answering the turn's message; tool progress is a `send`
  answering nothing whose lines grow by `edit`, each new line a step.
  Hermes edits it at most every 1.5 s, and upstream held a line that came
  sooner until a newer one, so a turn's last quick tool call showed no
  step; our image patches its sender to send the line once the interval
  is out (images/hermes/Dockerfile; the debt ledger).
- A progress line names its tool (`🖥️ computer_use: "capture"`, a
  terminal command's fenced block) or says its friendly verb (`🔍
  Searching the web for rust`, `✍️ Writing notes.md`: Hermes' own labels
  for its built-in tools, `display.verb` of its `locales/en.yaml`). One
  table of Hermes' tools (`relay/wire.rs`, `TOOLS`) reads either as the
  step's `tool` (Hermes' name for it), its `args` (the call's preview,
  its quotes and a repeat's count dropped) and its `category`; a tool not
  there is `other`, but for its browser's (`browser_…`). Hermes cuts a
  preview at its `display.tool_preview_length`, 40 unless set: our image
  sets it to 140, the most a step's `args` keeps (`hermes-boot`).
- Everything the agent says is a reply (Paul, 2026-10-24, in place of
  2026-10-05's one reply a turn). Hermes ends a draft segment at every
  tool boundary with a `send` answering the message, so the model's text
  beside a tool call ("Let me check that.") arrives as a reply, whatever
  its `display.interim_assistant_messages` (off in our image:
  docs/hermes-relay.md, "Hermes' settings"), and is one: whole (a
  reply's bound, never a step's 300 characters), with the safe markdown
  of any reply, posted before the step that follows, its draft put away
  as it posts. The engine marks it `interim` (the chat's push leaves it
  out: docs/chat-records.md, "Push") unless that step only updates
  memory: Hermes' answer written beside a housekeeping call, which it
  sends once and says nothing after, is the turn's answer. Interim
  replies have a bound of their own (`INTERIM_REPLIES_PER_TURN_MAX`, one
  a posted step), apart from `REPLIES_PER_TURN_MAX`, so narration never
  costs the turn its answer (`narration_never_costs_the_answer`). The
  tool's progress can reach the bridge before that `send`: a step that
  comes while a draft segment streams waits for it, at most
  `STEP_HOLD_MS` (3 s), the words then going before it; a question, a
  card, a file or the end stops the wait. And while a reply's image links
  are fetched (within the fetch's own 20 s) everything of its turn after
  it waits in order (`Inflight::queue`: its steps, a later reply, a card,
  a file, its end), so the reply keeps its files and the answer after it
  is never sealed by a step that waited (`later_parts_wait_for_a_replys_
  image`). A step whose words do not come in time posts, and the words
  after it, a reply of their own. Until 2026-10-24 the step took the
  words back as its `text`, at most 300 characters, which the page showed
  folded into the steps (Paul on production: "I can't read it all").
  Proven with the scripted gateway in every order (tests/relay.rs,
  `text_beside_a_tool_call_is_a_reply_of_its_own`, `many_narrated_calls_
  keep_the_answer`) and with the real Hermes (tests/docker.rs,
  `words_before_a_tool_call_are_a_reply`).
- An approval is a `prompt` op (`once`, `session`, `always`, `deny`);
  the owner's answer goes back at once as an inbound `prompt_response`.
  What asks at all is the image's (docs/computers.md, "Approvals in our
  Hermes image"): nothing technical (no command, file or code), so in our
  image only a saved payment card's fill; the agent asks its owner about
  outcomes with a clarify's card instead. Expiry needs no word: Hermes'
  `approvals.timeout` and `agent.clarify_timeout` are the card's life
  (`hermes-boot` gives the bridge the same; `HERMES_BOOT_CARD_TIMEOUT_S`
  is a test's shorter one). At an approval's timeout Hermes tries to edit
  the card (the bridge refuses: it is no message of the turn), sends `⌛
  Approval timed out …` (a notice), hands the model `BLOCKED: Command
  timed out without user response`, and the turn goes on to its reply; at
  a clarify's, it hands the model `[user did not respond within …]`.
- A notice is a `send` answering nothing that starts with one of the
  glyphs Hermes' gateway starts its notices with (`relay/wire.rs`,
  `NOTICES`, read from v0.21.6: `💾` its self-improvement review, `⚠️` its
  inactivity warning and a sub-agent's failure, `⏳` its busy acks and
  waits, `⌛` an approval's timeout, `✅`/`❌` a prompt's ack, its
  compaction's status, …): a `Notice` (`turn.notice`), the glyph its
  category, never a step. Hermes releases its review's notice once the
  turn is done, after its `✅`, so a notice with no turn running is the
  turn's the engine last ran in that chat, not a turn of its own. No
  tool's emoji is a notice glyph, so no progress line reads as one; and
  Hermes' word on a command said beside a turn (`⏩` a steer's, `💬 /btw:`
  a side question's answer) is the command's message of its own
  (above, "Commands"), read before any notice.
- A file Hermes sends is uploaded to `/relay/media`, then `send_media`;
  a message's attachments are served at `/relay/media/<id>`, behind the
  token.
- A file Hermes sends by a URL of its provider's (`send_media` naming
  one: a reply's image link, as FAL's image generation hands back) is
  fetched by the bridge (`relay/fetch.rs`), apart from the loop, and is
  the reply's attachment as an uploaded one is; the op is answered once it
  is in hand. Only `https`, from a host whose every address is public (no
  loopback, private, link-local or shared range, no `*.internal`), the
  connection made to the address checked; at most 25 MiB; a type the
  fragment serves as itself (an image, audio, video, a PDF); within 20 s
  (Hermes waits 30 s for an answer), at most 3 redirects, each checked as
  the first. Refused, the answer says why, and Hermes sends the link as
  text (its base adapter's `send_image`).
- A reply's image links are fetched the same way: Hermes sends a reply's
  `![alt](url)` (its URL an image's: an extension, FAL's or Replicate's
  CDN) as an image only from a reply it did not stream (`extract_images`),
  and ours all stream, so the reply carried the link as text. The bridge
  reads them as Hermes does (`wire::image_links`), fetches each, and puts
  it on the reply, the link taken out of its text; the turn's end waits for
  them (at most the fetch's 20 s). One not fetched stays a link.
- A question to answer in words: an open `clarify` (`❓ …`, the base
  adapter's text prompt), or `✏️ Type your answer:` after "Other" on a
  clarify's card, each read by its glyph (Hermes' translations keep
  them). It is a `send`: the bridge shows it as a reply part and
  the turn asks (`Asked`). Hermes blocks until the person's next message,
  which its gateway's clarify intercept takes even mid-turn, so the
  asker's next message goes back at once as an inbound in the same chat
  (`Tell`), not queued as a turn behind this one (which would wait out
  Hermes' clarify timeout, an hour).
- A clarify with choices is a `prompt` of `prompt_kind: "clarify"`
  (`❓ <question>`, its options `c0`… and `other`): its card shows the
  question without the glyph, and its "Other" is answered in words on
  the card (`words`: docs/chat-records.md), so typing an answer is one
  step, as replying is (Paul on p5, 2026-10-09). The words go to Hermes
  right after the answer, as an inbound in the same chat: Hermes sets the
  clarify to wait on words as it takes the answer, before it reads the
  next inbound, which its clarify intercept takes as the answer; its
  `✏️ Type your answer:` that follows shows nothing and asks nothing. An
  "Other" answered with no words (an older page) asks for them as above.
  The owner's message in the chat while the card is open is "Other" with
  those words, the same way: Hermes' clarify takes free text only once
  "Other" is chosen (its `mark_awaiting_text`; text that names no choice
  otherwise cancels the clarify and runs as a message of its own). Files
  sent with an answer reach Hermes as lines of its words (`Tell`, above):
  its clarify takes words alone.
- Stop is `interrupt_inbound` for the profile's session key. A clarify
  waiting on its person (their words, or its card of choices) never sees
  it, so a Stop while the turn asks, or while a clarify's card is open, is
  followed by the word `skip` (Hermes' own, which any clarify takes), which
  lets the wait go; Hermes may then reply once, its model told the
  question was skipped. Before this, a Stop on a turn waiting on a
  clarify's card closed the card, and the turn ran on until the clarify's
  hour was out (`a_stop_lets_its_clarify_go` in tests/docker.rs).
- **Commands** (above, "Commands"): the menu is Hermes v0.21.6's own
  manifest (`gateway/relay/command_manifest.py`, the names and
  descriptions its connectors register), less what changes the install
  or the gateway, or what the platform does its own way (`relay/menu.rs`,
  its table: `update`, `restart`, `sethome`, `reload-mcp`,
  `reload-skills`, `personality`, `title`, `resume`, `reset`, `help`,
  `approve`, `deny`, `thread`, `bg`). On the menu: `new`, `stop`,
  `steer`, `queue`, `btw`, `retry`, `undo`, `compress`, `model`,
  `reasoning`, `voice`, `usage`, `status`, `insights`. A command turn's
  inbound text is `/<name> <args>` as it is, which Hermes runs as its own
  command (never its model's words): idle, bracketed as a message is
  (`👀`, its answer as a reply, `✅`). `/stop` is never said: the bridge's
  Stop is. `/new` resets the session under the chat's own key (Hermes'
  `_handle_reset_command`: a new session id, the key the same; the
  overlay's `approvals.destructive_slash_confirm: false` asks nothing).
  In an agent's own chat the image's Bot Chat hook titles the new session
  `Bot Chat` as the next turn starts (docs/computers.md, "Bot Mode in our
  Hermes image"); until then the keeper holds the session before, still
  titled and the chat's, so a teammate's message still reaches the chat.
  `/model` and `/reasoning` change the chat's session alone: `--global`
  (and `reasoning`'s `show`, `hide`, which write the gateway's display
  settings) are refused.
- An aside (`/steer`, `/btw`) is an inbound in the agent's chat beside its
  turn, kept until acked like any: Hermes takes a command mid-run
  (`_handle_message_while_active`: every command it resolves bypasses the
  busy session). `/steer`'s words land in the running turn after its next
  tool call (`_busy_steer_command`). What answers either is a `send`
  answering the command's message id (`⏩ Steer queued …`, `💬 Side
  question: …`), and a side question's answer comes later answering
  nothing (`💬 /btw: "…"`, or `❌ /btw failed: …`, read by its glyph line):
  each said as a message of its own (`Event::Say`), never the running
  turn's. A steer that reaches Hermes just as its turn ends is queued by
  Hermes as a turn of its own, outside the bridge's (its reply then a
  message of its own too).
- A message quoting another carries Hermes' quoted reply
  (`reply_to_message_id`, the quoted record's turn id; `reply_to: {text,
  is_own}`), which Hermes puts before the message as `[Replying to: "…"]`
  (`[Replying to your previous message: "…"]` for the agent's own;
  `gateway/run_inbound.py`, `_prepend_inbound_reply_context`).
- The end: `👀` on, `👀` off, then `✅` or `❌`. A turn ends at its `❌`,
  at its `✅` once it said something, and, stopped, at its `👀` off.
  Hermes brackets a message it took while its gateway was starting
  twice, the first empty (docs/hermes-relay.md), and ends no person's
  turn without saying something, so an empty bracket's `✅` ends
  nothing. No clock: a turn whose bracket never ends is the idle
  bound's (`BRIDGE_TURN_IDLE_MS`: "the agent stopped answering").

Of decision 21's list, v0.21.5 has every op, but sends `task_card` only
for Slack chats (`gateway/run_turn.py`), so steps come from progress
text; `follow_up` is Discord's interaction tokens; neither is
advertised. It also has `thread_create` and `thread_rename` (threads are
off here).
