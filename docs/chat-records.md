# A chat's records

The contract between the bridge, which writes these records
(`images/bridge/src/records.rs`), and their readers: the chat template's
page and push (`templates/chat`), the shell (a chat's listed turn kinds,
its delegation threads), and a person's export (`transcript.md`,
`fragment_core::export`). Decisions 8, 9, 38 and 42.

A chat is a fragment with two channels and one job of the template's
code:

```json
"channels": {
  "chat": { "read": "public", "post": "viewer" },
  "work": { "read": "viewer", "post": "editor", "from": "agent",
            "listed": { "kinds": ["turn.start", "turn.step", "turn.prompt",
                                  "turn.prompt.closed", "turn.asked", "turn.end"],
                        "fields": ["asks", "outcome"] } }
},
"operations": { "notify_reply": { "kind": "job" } },
"triggers": [{ "channel": "chat", "from": "agent", "run": "notify_reply" }]
```

- `chat` holds what is said: people's messages, agents' replies, Stop,
  prompt answers, and an agent's owner's commands.
- `work` holds an agent's progress: its turns, steps, cards and menu.
  Only the chat's agent members post there, each as itself (`"from":
  "agent"`), so no person can forge a step, a card or a menu. Its turn
  kinds are `listed` (docs/api.md, Listed records): each agent's newest,
  with its `asks` and `outcome`, rides the chat's row, so the shell says
  what an agent is doing without reading `work`.

An agent fragment has one more channel, which its computer follows:
`"tasks": { "read": "editor", "post": "editor" }` (below).

Each body below is a record's `body`. The record also carries `seq`,
`at` and `principal`. Ids are each poster's own: the same id and body
again append nothing, and another body under that id is a 409. A reader
attributes a record by its `principal`, never by what its body says. A
turn belongs to the agent whose `turn.start` began it, and its later
records count only from that agent.

## Turns

A **turn** is what one agent does about one record. Its id is the first
24 hex digits of SHA-256 of `<agent fragment>|<fragment>/<channel>/<seq>`.
So a record read twice is one turn, and two agents answering one message
are two turns. When the runtime says something with no turn running, that
is a turn of its own, numbered by the bridge's life and counter.

A turn a restart cut may be followed by its **continuation**, a turn of
its own: its id is the first 24 hex digits of SHA-256 of `continue`, a
NUL and the cut turn's id; it has the cut turn's `cause` and asker, and
a `turn.notice` (part `continued`) as it starts (docs/bridge.md, "A cut
turn is continued once").

One turn of an agent runs in a chat at a time; the rest wait in order, at
most 16. A turn's records are its `turn.start` on `work` (its claim), any
number of steps, replies, cards and their closings, `turn.asked` and
notices, then its `turn.end` on `work`. Every turn has exactly one start
and one end, including a refused turn and one stopped while it waited
(docs/bridge.md, "State").

While a turn writes a reply, the chat's **draft** holds the reply's whole
text so far (`PUT …/channels/chat/draft {turn, text}`; never stored;
`text: null` stops it). A page shows it after the turn's last record,
until a reply of that turn replaces it.

## `chat`

**A person's message:**

```json
{ "text": "hi @juniper", "to": ["npub1…"], "attachments": [ATTACHMENT], "reply_to": 12 }
```

- `text`: at most 32 KiB (more is cut). `kind` is absent or `"message"`.
  A body that is a bare string is a message with that text.
- `reply_to`, optional: the `chat` seq of the message it quotes, a
  positive integer (anything else makes the body no message). The
  runtime is handed the quoted text, and whether it is the agent's own.
- `to`, optional: the agents it is for. Without `to`, an agent answers
  when the text `@mentions` its name or its fragment's label. Otherwise
  the **lead** answers: the first agent added, by `addedAt`.
- An anonymous visitor's message starts nothing. Neither does a message
  posted before the agent joined.
- A turn may ask its asker something to answer in words (`turn.asked`).
  The asker's next message to that agent is then the answer, not a turn
  of its own. A turn may wait on a card that takes words. The owner's
  next message to that agent then answers the card, with that option and
  those words. Files go with either answer. Anyone else's message waits
  its turn, and so does any message while a card takes no words.

**An agent's reply:**

```json
{ "text": "…", "turn": "<turn>", "attachments": [ATTACHMENT], "to": ["npub1…"], "hop": 1, "interim": true }
```

- Its id is `rp:<turn>:<n>`, numbered from 1. Each reply is whole;
  `text` is at most 56 KiB. Everything the agent says is a reply,
  including its words before a tool call, which are posted before that
  call's step.
- `interim: true` marks words its turn went on working after: words
  before a tool call that does more than update memory. The push leaves
  an interim reply out; a page shows it as any reply. A turn posts at
  most 64 replies that are not interim and, counted apart, 200 interim
  ones, so narration never costs the answer.
- `to` and `hop` are set on a hand-off: a reply that `@mentions` another
  agent of the chat. An agent answers another agent's record (a reply,
  or a message an agent posts itself: `fragment ask`) only when its `to`
  names that agent, and within the answering bridge's hop count and the
  chat's budget; a body's `hop` only raises the count. A hand-off past
  the budget ends with an `error` saying so (docs/bridge.md, "Agents
  asking each other").

**A relayed message**: its owner's words through one of the agent's
doors (docs/api.md, "An agent's doors"), posted in the chat that door
mirrors:

```json
{ "text": "…", "to": ["npub1<agent>"],
  "relayed": { "via": "simplex", "for": "npub1<owner>", "agent": "<agent fragment>", "at": 1790000000000, "sig": "<128 hex>" },
  "attachments": [ATTACHMENT] }
```

- `via` is the door's id (a connector's: `simplex`, `telegram`); `for`
  is the agent's owner; `agent` is the agent fragment whose door it came
  through. The platform takes a relayed record only through a door its
  owner opened, and no other.
- SimpleX's (docs/bridge.md, "SimpleX") is posted by the agent on its
  SimpleX chat. Its id is `sx:<first 32 hex digits of SHA-256 of "<agent
  fragment>|<the sender's message id>">`, so a message read again is one
  record. `at` and `sig` are its door's signature: the agent relays,
  never posts as its owner.
- A hosted door's (a connector chat) is published by the connector's
  own app, as its own key, in its own `chat`, its `relayed` exactly
  `{via, for, agent}`.
- Only that agent's own bridge honors it, and only when it is carried by
  the agent itself or by a key that is none of the chat's members: it is
  the owner's message, asked as them, whether or not a link is bound now.
  Any other `relayed` is read as the record without it: another `for`,
  another agent's door, a `via` that is no door's id, or carried by
  another member.
- A page shows it as the owner's, "via" its door, once the members say
  its poster is their agent or none of the chat's members.

**An ask** (`fragment ask --return-to`): an agent's message to another
agent of its computer whose answer comes back to it, as a turn of its
own, in the chat it asked from:

```json
{ "text": "What's on Friday?", "to": ["npub1<asked>"], "return_to": "<chat it asked from>" }
```

- The question is a message like any; `return_to` is the whole of the
  return. Only another agent of the answering bridge's computer asks so;
  anyone else's is a plain message, as it is to a bridge before asks.
- Once the asked turn is over, its bridge posts the **answer** on the
  asking agent's `tasks` (below), however long that takes and whether or
  not the asking agent still runs. Its outcome is the turn's end's: a
  turn a restart cut, or another life ran, is answered `error`, never run
  again. An asked turn that asks back in words is told at once that no
  answer is coming, and answers as best it can.
- A question refused as it comes (past the hops, the chat's budget, or a
  full queue) is answered `refused`.

**Stop:**

```json
{ "kind": "stop", "turn": "<turn>" }
```

A page posts it with the id `stop:<turn>`. Only the turn's asker stops a
turn (for a routine or an ask's answer, the agent's owner). Without
`turn`, it stops the asker's running turn. A waiting turn it names never
runs.

**A prompt's answer:**

```json
{ "kind": "prompt_response", "prompt": "<prompt>", "option": "<option>",
  "agent": "npub1…", "turn": "<turn>", "text": "purple" }
```

Only the agent's owner answers (decision 42), and the first answer wins.
`agent` and `turn` name the card, both or neither: prompt ids are each
agent's own. An answer naming neither is matched by its prompt alone. A
page's id is `pr:<first 32 hex digits of SHA-256 of "<agent>
<turn>:<prompt>">`, so a second tap is the same record (or a 409).
`text` is only for an option answered in words: what was typed, at most
32 KiB. Without it, such an option is asked for its words.

**A command**, from an agent's owner to its runtime:

```json
{ "kind": "command", "command": "steer", "args": "use the blue one", "to": ["npub1…"] }
```

- `command` matches `^[a-z0-9_-]{1,32}$` and is on the agent's menu
  (`commands`, below). `args` is optional, at most 32 KiB. `to` is as a
  message's. Any other shape is no command, and starts nothing.
- A command from anyone but the owner, or one off the menu, is passed
  over. A message whose text starts with `/` is words, never a command.
- The menu says how each command is carried (`runtime::How`): most are
  a turn of their own, the runtime handed `/<command> <args>` as typed;
  some stop the agent's turns in the chat, whoever asked them, or go
  into its running turn at once. What answers a command said beside a
  turn is a message of its own. A command the menu refuses for a word of
  its `args` still gets both its records, its end an `error` saying why.

**History**, a chat continued from a conversation its person had (one of
their Session Archive, templates/sessions/README.md, or an archived chat
of theirs), with an agent of their choosing:

```json
{ "kind": "history", "title": "Beans", "archive": "sessions--k3x9", "key": "hermes-20261001_090000_ab12cd",
  "agent": "Juniper", "started_at": "2026-10-01T09:00:00.000Z", "ended_at": "2026-10-04T09:00:00.000Z",
  "summary": "…", "messages": [{ "role": "user", "at": "…", "text": "…" }], "earlier": 12 }
```

- Posted by the chat's owner (the shell's Continue, cell/shell/continue.js)
  with the id `history`, before the agent joins: the chat's first record.
  It holds the conversation's last summary and the newest messages said
  after it (the summary covers what came before), oldest first, with how
  many others the conversation holds (`earlier`), bounded to fit one
  record (`historyOf`, `historyOfChat`). `archive` and `key` name an
  import's file, or `chat` the archived chat it was made of. `agent`:
  whose the agent's words were, when not the agent continuing it.
- One chat a (source, agent): its name is made from both, so Continue
  asked again, in any tab, opens it, and a Continue cut short is finished
  in the same chat.
- Only its owner's counts. The page shows it above the chat, with whom it
  was (`agent`). The bridge hands it to the agent once, as its first
  turn's note there (`note::history`, another agent's words by its name);
  it starts no turn.

Any other `kind` on `chat` is a page's own, never a message. Search finds
messages and replies alone, by their `text` (docs/api.md, Search).

## `work`

The agent posts each record with the id `wk:<turn>:<part>`, so a step
posted again is the same record.

**`turn.start`**, part `start`:

```json
{ "kind": "turn.start", "turn": "…", "asker": "npub1…", "agent": "npub1…",
  "cause": { "fragment": "…", "channel": "chat", "seq": 12 },
  "life": "<32 lowercase hex>" }
```

`asker` may stop the turn. `agent` is the agent's identity. `life` is the
bridge process that claims the turn. The record is the claim, "this life
runs this turn" (docs/bridge.md, "State").

**`turn.step`**, part `<step>`, from 1: one tool call.

```json
{ "kind": "turn.step", "turn": "…", "step": 1, "tool": "terminal",
  "category": "shell", "args": "ls -la", "ok": true, "excerpt": "Exit code: 0",
  "tool_call_id": "call_…", "status": "completed", "duration_ms": 120,
  "response": "…" }
```

`tool` (the runtime's name for it) and `args` (the call's preview) are at
most 140 characters each; `excerpt` is at most 300. A turn posts at most
200 steps. `category` is one of `shell`, `web`, `read`, `write`,
`browser`, `image`, `delegate`, `memory` or `other`; a reader shows any
other category, or none, as `other`.

`tool_call_id` (at most 128 characters), `status` and `duration_ms` say
the call as its runtime observed its end: `status` is `completed`,
`failed`, `timed_out`, `cancelled` or `blocked`, and `ok` is true for
`completed` alone. A step with them has ended; its `excerpt` is a
command's exit code or empty, never its output. `response` names the
model response whose calls were running, as its `turn.call`s do. Older
bridges wrote steps without them, whose `ok` was true even for a call
that failed, and wrote `text`: the model's words before the call, which
a page shows as a reply.

**`turn.call`**, part `call:<first 24 hex digits of SHA-256 of its
response, a NUL, and its place in it, from 0>`: a tool call the model
asked for, said before it runs.

```json
{ "kind": "turn.call", "turn": "…", "response": "…", "tool_call_id": "call_…",
  "tool": "terminal", "category": "shell", "args": "make deploy" }
```

`response` (a post id) names the model response that asked for it; its
place there makes it one call, so a call said again is one record.
`tool`, `args` and `category` are as a step's, and a turn posts at most
200 calls. It is the call's intent, no more: a refusal may still stop it,
and only a `turn.step` of the same `response` says it ended, the one with
its `tool_call_id`, else (its runtime renamed the call) the one with its
`tool` and `args`. The provider's `tool_call_id` is no call's identity:
some name every response's first call `call_0`. A call no step ended is
named in the cut turn's note as one that may or may not have run. Pages
show none yet; older bridges post none.

**`turn.prompt`**, part `p:<prompt>`: a card, a question in plain words.

```json
{ "kind": "turn.prompt", "turn": "…", "prompt": "<prompt>",
  "text": "Make your recipe app public, so anyone with its link can open it?",
  "options": [ { "id": "c0", "label": "Yes, make it public", "style": "primary" },
               { "id": "c1", "label": "No, keep it private" },
               { "id": "other", "label": "Something else", "words": true } ],
  "asks": "id:<owner>", "expiresAt": 1791000000000 }
```

`prompt` matches `^[A-Za-z0-9._-]{1,64}$`, and option ids the same with
at most 32 characters. A card has 1 to 8 options; `style` is optional
(`primary`, `danger`). An option with `"words": true` is answered in
words on the card (the answer's `text`, above). Only `asks` answers, and
only until `expiresAt` (ms). A card outlives a restart of its turn: an
answer whose turn is gone starts a fresh turn (docs/bridge.md, "A card
outlives its turn").

**`turn.prompt.closed`**, part `pc:<prompt>`:

```json
{ "kind": "turn.prompt.closed", "turn": "…", "prompt": "<prompt>",
  "outcome": "answered", "option": "c0", "by": "npub1…" }
```

`outcome` is `answered` (with `option`, `by`, and `text` for words),
`expired` (unanswered by `expiresAt`), `stopped`, or
`history_unavailable` (a late answer whose card's journal could not be
read whole: nothing ran). A card answered late also names the answer's
seq on `chat` (`answerSeq`); one answered live names none.

**`turn.asked`**, part `q<part>`:

```json
{ "kind": "turn.asked", "turn": "…", "asks": "npub1…" }
```

The turn asks `asks` (its asker) something to answer in words, reply
`<part>` being the question, and waits, running, for their next message
to the agent. It is posted once per question. Nothing records the answer:
a page takes the turn as waiting until `asks` posts a message on `chat`
at or after the record's `at`, or a later record of the turn's on `work`
says it went on.

**`turn.notice`**, part `n<k>`, from 1:

```json
{ "kind": "turn.notice", "turn": "…", "category": "memory",
  "text": "Self-improvement review: Memory updated" }
```

A notice from the runtime, neither a step nor a reply. `category` is
`info`, `warning` or `memory` (a reader treats any other as `info`).
`text` is at most 300 characters, and a turn posts at most 32 notices. A
notice the runtime sends after the turn's end still belongs to that turn,
and is posted after its `turn.end`. If the agent has run no turn in that
chat in this life, the notice is not posted. A notice is never pushed.

**`turn.end`**, part `end`:

```json
{ "kind": "turn.end", "turn": "…", "outcome": "idle" }
```

`outcome` is `idle`, `stopped`, or `error`, with `error` (at most 300
characters) saying why: the runtime failed, it stopped answering, the
turn was `lost when the computer restarted`, or it was refused.

**`commands`**, the agent's **menu**:

```json
{ "kind": "commands", "agent": "npub1…",
  "commands": [ { "name": "usage", "description": "Show token usage for this session" },
                { "name": "steer", "description": "Inject a message after the next tool call (no interrupt)",
                  "args": "What to tell the agent" } ] }
```

Each entry is a command's name, what it does, and what its words are
(`args`, absent for a command that takes none). The agent posts it with
the id `cm:<life>`, as it first runs a turn in the chat in a life, so a
page's window of `work` holds it while the chat is in use. A reader takes
an agent's latest menu, and only one whose `agent` is its poster. A
runtime with no commands posts none.

After a lost turn, and after a rollback, the agent's next turn is told
what it missed, from these records alone (docs/bridge.md, "Restarts").

## Attachments

```json
{ "sha256": "<64 lowercase hex>", "size": 12345, "type": "image/png", "name": "cat.png" }
```

A file is one of the chat fragment's blobs (docs/api.md, Blobs). Its
poster uploads it first, then posts the record naming it, which keeps the
blob. A record has at most 8 files, each at most 25 MiB. The agent's
runtime gets each as a local file, named by its `name` and `type`
(docs/bridge.md, "Hermes' Relay").

**A voice memo** is an attachment whose `type` is audio, in the format
the recording browser made it: `audio/webm`, `audio/ogg` or `audio/mp4`.
SimpleX's are `audio/mp4`. The platform does nothing with the audio: the
agent's runtime transcribes it, if it does.

## The page

The chat template's page is `templates/chat/site/`. Beyond the shapes
above, it relies on:

- **The window it reads**: the last `CHAT_LAST` records of `chat` and,
  for a viewer, the last `WORK_LAST` of `work` (the reason a menu is
  posted again each life).
- **The launcher's chat**: `?sent=1` shows no new chat's start, only the
  message just sent and its waiting line.
- **The first run**: `?setup=1` shows the making as the shell reports
  it, and sends nothing until an agent is a member.
- **Its frame's messages** to and from the shell (docs/api.md, The
  shell): among them its owner's other agents in its `@`, which a message
  adds first, and "Open" beside a link to another fragment's page.
- **Its presence**, `{typing, looking}`: `looking` while the chat is on
  screen, which the push reads.
- **Who is who**, from the fragment: `__members` (the lead first) and
  `__people`. An agent shows by its agent fragment's title and picture.

## Push

The template's job `notify_reply` (`app.mjs`) runs for each record an
agent of the chat posts on `chat`. It pushes each reply that is not
`interim` to each of the chat's people whose page does not say
`looking`. It pushes by identity (docs/api.md, Deliveries), under the
chat's tag. A member who calls the job directly pushes nothing.

## `tasks`

**A routine** (decision 38):

```json
{ "kind": "routine", "text": "Water the plants", "chat": "<chat fragment>" }
```

A routine is a scheduled record on the agent fragment's `tasks`
(docs/api.md, "Scheduled records"; `fragment schedule add --routine`).
Each firing wakes the computer. The agent takes it as a turn in `chat`,
asked by its owner, at no hop. With no `chat`, it runs in the owner's
home as it fires (docs/api.md, "An agent's home"). A routine older than
an hour when first read is skipped, as cron skips a missed run. A routine
with nothing to tell may post no reply (docs/bridge.md, "A routine with
nothing to tell says nothing").

**The agent's hello.** Once the shell has made an agent and added it to
its chat, it posts a routine for that chat as the agent's owner, with the
id `hello`, asking the agent to say hello. So the agent speaks first.

**An ask's answer** ("An ask", above):

```json
{ "kind": "answer", "chat": "<chat it asked from>", "text": "Rowan answered your question in <chat> (\"…\"):\n\n…",
  "by": "npub1<asked>", "name": "Rowan", "asked": { "fragment": "<chat>", "seq": 12 }, "turn": "<asked turn>",
  "outcome": "idle", "error": "…", "hop": 2, "refused": true }
```

- Posted by the asking agent's own bridge, as that agent, with the id
  `an:<asked turn>`, so one question has one answer whatever life posts
  it. Only the agent's own key's answer is taken.
- The agent takes it as a turn in `chat`, asked as the agent that
  answered (`by`, called `name`), and its owner's to stop as a routine
  is. `text` names the question and carries what the asked turn said
  (its replies but interim ones, bounded); `outcome` and `error` are its
  end's.
- The turn is `hop` hand-offs deep, one past the turn that answered, and
  spends `chat`'s budget. `refused`: the question was never taken, and
  this turn is recorded refused, never run.
- An answer is taken however late it is first read: the hour that skips
  a routine never skips an answer.

Only the agent fragment itself (its cron, the platform's `joined`) and
the agent's owner ask anything on `tasks`, and the agent's own bridge
posts its answers; any other poster is passed over. Another of the
owner's agents may post there, but its routine would be a hand-off no
hop count holds: it asks in a chat instead.

**Joined:**

```json
{ "kind": "joined", "fragment": "<fragment>" }
```

The platform posts it, as the agent fragment itself, once, when the agent
becomes a member of `fragment`; the computer follows that fragment at
once. Whatever adds an agent needs to do nothing more.
