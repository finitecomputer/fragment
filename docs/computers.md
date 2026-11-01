# Computers: the platform's contract with an image

The platform side is the Computer Durable Object (`cell/src/computer.rs`),
whose lifecycle is the pure state machine `fragment_core::computer`; the
wire types are `fragment_proto::computer`, and its owner's routes are
docs/api.md, Computers. The image side is any image: ours runs Hermes
(`images/hermes/`), and the stub (`images/stub/`) proves the platform
needs nothing Hermes-specific. Nothing here names an agent runtime; a
change that would have to is a design bug. The process our images run
between a runtime and the platform is the bridge (docs/bridge.md).

## What a computer is

- One per person for now (decision 13): `computer:<24 hex>`, its id
  derived from its owner, made asleep by `POST /api/computers`. It runs
  one image, pinned per computer (decision 19), at the price book's
  instance (`fragment_core::price::INSTANCE`).
- Its agents are fragments of kind `agent` assigned to it (decision 14),
  at most 16. An agent's identity is its agent fragment's npub (decision
  45); the computer signs as an agent only while it is assigned there.
  The guest acts *as* its agents, with at most their grants, never as its
  owner (decision 36). An agent fragment deleted leaves its computer as
  an unassign does.
- The Computer DO is the only thing that talks to its container: it
  starts and stops it, saves and restores `/data`, runs its egress
  intercepts, proxies its ports, and wakes it. It never reads what the
  image runs.
- Its owner's export holds its view and its newest save that restores
  (docs/api.md, Export). It is deleted only with its owner, by an
  operator's wipe (docs/api.md, "Operators: wiping a person"), saves
  included. Cloudflare has no delete for a container snapshot: a wiped
  computer's is forgotten, never restored, and expires within 30 days.

## Lifecycle

Phases: `asleep`, `starting`, `awake`, `sleeping`, and `wont_wake`
(lesson 4). `fragment_core::computer`'s module doc is the whole machine;
an image may rely on this:

- **It is woken** by a record on a channel one of its agents subscribed
  to with `wake: true`, one of its agents added to a fragment, an open
  port tab, a page's presence on a subscribed fragment (a pre-wake, held
  a minute), or its owner. Never by its own guest's traffic.
- **It stays awake** while a port tab is open or the guest holds its
  keepalive socket, and `IDLE_MS` (20 minutes) after neither does. An
  always-on seat's computer never sleeps (decision 25; docs/billing.md).
- **It is saved** when its work ends (its last keepalive closes and 30 s
  pass with none opened again), every 15 minutes while a keepalive stays
  open, and at every sleep (below, "Saves and what a wake restores").
- **A sleep** is, in order: the hold, the save, a container snapshot
  (only after a save that worked), SIGTERM, up to 5 s for the guest to
  exit, destroy. A keepalive opened during an idle sleep's hold cancels
  the sleep. A sleep whose save fails keeps its container and is tried
  again; after `computers.unsaved_max_ms` (30 minutes unless the
  deployment says otherwise) of failed tries it sleeps unsaved, and its
  next wake is a rollback.
- **A restart** (its owner's) is a sleep that saves if it can and stops
  whether or not it did, then a fresh start: from the image and the
  newest good save, never the snapshot, which could carry what broke it
  outside `/data`.
- **A crash** (the container stopped on its own) starts it again while
  something wants it; a guest that died holding its keepalive is started
  again however long its turn ran, and its next life ends what was cut.
- **Won't wake**: after `STARTS_FAILED_MAX` failed starts in a row it
  starts no container until its owner asks.

## Durability

**The rule** (Paul, 2026-10-05; decision 18 points here): one authority
for each fact. Everything else is a cache, and a cache may be lost or go
back in time without changing what happens.

| Fact | Its one authority | A cache of it |
|---|---|---|
| What was said | the chat's `chat` channel | the runtime's transcript |
| Which turns started, which calls they asked for, what they did, how they ended, each card's answer | the chat's `work` channel, the journal (docs/chat-records.md) | the bridge's `state.json` |
| The agent's self (`SOUL.md`, memories, skills) | the agent fragment's repo | the profile's checkout |
| The runtime's context and the agent's working files | `/data`, as of its newest save | the snapshot, for one image |
| Which save is current, and what a wake restored | the Computer DO | |
| Usage | the ledger | |
| A messenger's encryption state | SimpleX's core's databases in `/data`, as of the newest save (docs/bridge.md, "SimpleX") | |

A guest holds its keepalive only for live work: a turn queued or
running, or delegated work still running after its parent replied
(decision 65; docs/bridge.md, "Runtime work outside a turn"). Waiting on
a person never holds it. That work is never saved: a restarted runtime
has none of it.

- **P1, one life per turn.** A turn runs only in the life of the bridge
  that journaled its start, once the platform answered that post. A
  rolled-back or lost `/data` runs no turn twice, and a turn a crash cut
  ends once, as lost: a second run of a turn with effects (an email sent
  twice) is worse than a turn the person is told was lost.
- **P5, the turn after a cut one is told.** The agent's next turn in that
  chat is told what was cut, from the journal alone (docs/bridge.md, "The
  turn after a cut one is told"): first each tool call it asked for that
  no step ended, which may or may not have run, since each call the model
  asks for is recorded on `work` (`turn.call`: docs/chat-records.md).
- **A cut turn is continued once** (durability phase 4, Paul,
  2026-10-31: nobody should type "continue"). A turn this life ended as
  lost is queued once more in its place, said by the computer and
  carrying the cut turn's note; its runtime is never asked to rerun a
  call. Never twice, never a continuation's, and never one its asker
  stopped or one that waited on its person (docs/bridge.md, "A cut turn
  is continued once").
- **A late message is told its age** (Paul, 2026-10-31). An agent
  handed a message late (a backlog after an outage, a restart or a
  sleep) is told how old it is, and judges whether it is still wanted;
  nothing is cut off (docs/bridge.md, "A late message is told its age").
- **P6, a card outlives its turn.** A sleep or a restart may cut a turn
  under its card. The card stays answerable until it expires; a late
  answer starts one fresh turn, claimed by the answer, with the card's
  turn as its note, and the old turn never resumes (docs/bridge.md, "A
  card outlives its turn (P6)"). Not yet proved by a hosted run's real
  sleep and wake: the Docker lane's sleep is a save and a restore.

**A+ now, toward E** (Paul, 2026-10-05). A save asked of the guest covers
only the writers it knows of, and some state must never go back in time
(a messenger's ratchets), which no save of a disk that can be lost gives:
Cloudflare restarts hosts, always-on containers included. The decision,
in steps:

1. **A+**, Cloudflare's intended design (built): save when idle, on a
   timer while busy and at every sleep; the guest copies its databases
   under the hold; three saves kept; a wake falls back past one that will
   not restore.
2. **The seam** (built): `/data/work`, what the tools write, apart from
   the rest of `/data`, the runtime's own, each saved as a record of its
   own (below, "Data and the restore gate").
3. **Our terminal backend** (not built): Hermes runs its tools' commands
   through a terminal-backend plugin of ours, at first in the same
   container, so the platform knows when tools are busy, and execute_code's
   temp files join the work.
4. **E**, Hermes' intended design (not built): that backend runs commands
   in a Cloudflare Sandbox of the computer's own, whose Durable Object
   saves its disk when idle; the gateway's container keeps only the
   runtime's home. To settle first: each tool call's added round trip,
   where the browser and desktop live, and the swap and the `fragment`
   CLI inside the sandbox.
5. **Toward C**: each piece of the runtime's home moves to an outside
   authority, until `/data` holds nothing authoritative.

Not built either:

- **F**, messengers outside the computer: a connector Durable Object per
  linked account holds the protocol's state, so it never rewinds.
  Deferred (Paul, 2026-10-05: "too much of a lift for now"); SimpleX runs
  on an always-on computer meanwhile (decision 32).

## What its owner is told

Paul, 2026-10-08: no one's computer may die unexplained while they use
it, and each needs a way back to working.

The owner's view carries `notices` (proto's `ComputerNotice`, from
`fragment_core::computer::notices`; their fields are docs/api.md's), the
most pressing first. Each is derived from the Computer DO's own state
with no clock, so every reader is told the same, and when they change the
DO tells its owner's open pages (`/api/fragments/watch`'s `changed`):

- `wont_wake`: its starts kept failing.
- `unsaved`: this life's saves are failing (a sleep's at once, awake ones
  from the second in a row): since when its work is in no save, and when
  the bound stops it.
- `went_back`: a start went back to an older save, or will at its next
  start, told until its owner says they saw it. None when nothing was
  done on the lost life after that save (Paul, 2026-10-23).
- `update`: it is awake, with no keepalive open, on another version than
  its next start runs, until its owner puts that version off.

The platform posts none of them in a chat: a chat's records are its
people's and agents' (docs/chat-records.md). The shell shows them over
every chat and Settings.

- **One way back: Restart** (`POST /api/computers/{id}/restart`): the
  sleep and fresh start above. It names the start its owner saw, so
  pressed twice it restarts once, and the ledger is asked first, as for a
  wake.
- **An update is told, never forced** (Paul, 2026-10-09). A deploy that
  changes an image pushes it under the same name: asleep, a computer takes
  it at its next wake; awake, it runs the image it started on until it
  sleeps or its owner restarts it. A version is the image digest's first
  12 hex digits (`version_of`), so a deploy that rebuilt nothing tells
  nothing.
- **The bound never stops a computer in use.** It stops one only at a
  sleep, which waits while anything holds it, so it caps how long a
  failing idle computer is kept. That time is the platform's, not metered
  (docs/ledger.md, "Who pays").
- **The agent** is told what a cut or a rollback lost, in its next turn
  in each chat (docs/bridge.md, "The turn after a cut one is told" and
  "What a rollback forgot").
- **Getting help**: a deployment may name `support_url`, which the shell
  links once a restart has failed too.

## Preview and production

**There's preview and there's prod** (Paul, 2026-10-31). One image name,
`hermes`: preview runs master's image, and production runs the digest
preview proved, promoted by Paul.

- **Pins are digests.** Every image a CI config names is pinned at
  `…@sha256:<64 hex>` (deploy.rs's `pinned_ok`); tags only find them. A
  computer is pinned to an image by its name in the config (`hermes` by
  default). A computer pinned to a name the deployment no longer has has
  no image to start: move its pins (`fragment operator pin`) before a
  name goes.
- **Preview follows master.** Each green master push deploys, as
  `hermes`, the image its checkout's inputs make, reused by its input key
  when it was built before (`xtask/src/image/preview.rs`). The merge
  queue builds it first (`preview-image.yml`, under queue tags, never
  deploying and never a required check); master's job alone writes an
  input tag.
- **What changes a version.** Only the image's inputs (Paul, 2026-10-14:
  a change to the cell or the shell needs no image rollout); a host test
  holds that list to the Dockerfile.
- **Production stays pinned** (`production.jsonc` in
  finitecomputer/fragment-deploy) to a digest a round proved, copied byte
  for byte into production's registry (`cargo xtask image copy`), never
  rebuilt. Only Paul promotes.
- **Guest-facing contracts stay additive.** The cell can be ahead of a
  computer for days (an awake computer keeps its image across a deploy),
  and production's image lags master by a round.
- **A Hermes release is forward only** (Paul, 2026-10-13). A change only
  to our layers on the same Hermes base can be pinned back. A new Hermes
  release may advance its databases' schemas, and a computer's three
  saves soon hold none from before it, so it is fixed forward
  (docs/technical-debt-ledger.md, "A Hermes release has no rollback").

### One round before production

1. Record the input key and digest from master's preview deploy summary,
   and the running reference of each computer watched (a later push may
   move preview).
2. Live on it: an old chat continues, a new one recalls a memory, skills,
   a routine, the screen and Take over, Google, a teammate's ask answered
   back. Watch `version == latest`, `rollbacks` unchanged, every save
   `held`, no `wont_wake`, `unsaved` or `went_back`, operator health,
   logs and spend.
3. Run the hosted lanes on preview, then `--sweep`: `cargo xtask e2e
   --hosted --config <preview config> --branch preview --only
   computers,agent-restart,agent-smoke,agent-ask,bot-mode
   --max-paid-calls 190`.
4. Prove the upgrade from production's image: name production's digest
   in the preview config as an image of its own (`production`, in the
   preview's registry, which a promotion copied it from), let the next
   deploy take it, and run `--upgrade-from production --only
   agent-restart --max-paid-calls 16`, then `--sweep`. Remove that line
   once no computer is pinned to it.
5. Soak the recorded digest for 24 hours, the watched computers having
   answered on it and the hosted runs green. A later image is another
   round.
6. Paul promotes it: the Image workflow's production copy (`from` the
   proven reference), its pin committed to `production.jsonc` with the
   old line kept for rollback, then a move of `production`.

## The guest's contract

What every image may rely on, and must do.

### Environment

| Variable | Value |
|---|---|
| `FRAGMENT_COMPUTER` | its id, `computer:<hex>` |
| `FRAGMENT_API` | `http://api.fragment.internal` |
| `FRAGMENT_MODEL` | `http://model.fragment.internal` |
| `FRAGMENT_IMAGE` | the pinned image's name and digest |
| `RESTORE_PENDING` | `1` when `/data` is being restored |

The guest holds no credential. Every request to those hosts, and to a
provider's host (below, "Connections and operator keys"), goes through
the Computer DO's intercepts, which add what the platform holds; other
traffic goes out as it is (decision 43). HTTPS to an intercepted host is
signed by Cloudflare's containers CA, which the runtime puts at
`/etc/cloudflare/certs/cloudflare-containers-ca.crt`: an image trusts it
wherever its tools read trust (our Hermes image: the system bundle,
Node's, and its Chromium's policy).

### Its process

- The entrypoint is PID 1 in a PID namespace of its own
  (`containers_pid_namespace`). PIDs repeat from one start to the next: a
  lock that names a PID from before a sleep can name a live process after
  it.
- As it starts, the image reads its own view (`GET /api/computer`),
  retrying at least every 30 s until it answers: that read is how the DO
  knows a start from a snapshot came up alive (#362). One unheard within
  `ALIVE_MS` is destroyed, and the same wake starts the image and its
  save.
- A snapshot keeps the files, not the processes: a start from one runs
  the entrypoint again.
- SIGTERM means stop now: flush and exit within 5 s. `/data` was saved
  before the signal; whatever is written after it may be lost.

### What every image carries

- `/usr/local/bin/sandbox-shim` from `cloudflare/sandbox:1.0.0`: the DO's
  `DirectoryBackup` saves and restores `/data` through it.
- `sh` (with its `test`), `true`, `mkdir`, `touch`, `rm` and `cat`, with
  which the DO polls, marks and reads its markers.

### The hold

A handshake before every save. The DO removes `/run/computer/held`,
touches `/run/computer/hold`, and waits up to `HOLD_WAIT_MS` (20 s) for
the guest to touch `/run/computer/held`: its answer that it claims
nothing now (no claim in flight, none new while the hold exists) and has
copied what a hot copy would tear. Then the DO saves. An awake save lets
go after (it removes the files); a sleep keeps the hold until its
container is gone. A guest that never answers is saved anyway, the save
recorded as not `held`. What the guest already claimed may run on. A
fresh container never has the files: the DO removes them from one started
from a snapshot.

- **What the save leaves out.** `held` may name, one per line, what the
  save of `/data` leaves out: gitignore patterns relative to `/data`, the
  files the guest copied under the hold to names the save keeps, so a hot
  copy of one is never what a wake restores. Bounds and the patterns'
  alphabet are `fragment_core::computer::left_out`'s. An empty answer
  leaves nothing out; one the DO refuses saves the guest whole.
- **A hold not answered.** Until it answers, a guest may say why in
  `/run/computer/unheld` (at most 1 KiB of text), which the DO reads only
  once its wait ran out and logs with the hold. Nothing is decided by it.

### Data and the restore gate

- `/data` is the only directory kept across sleeps. The image does not
  ship it: a restore swaps a restored directory into place, which
  overlayfs refuses for one from an image layer (EXDEV); on a first start
  the image makes it.
- **The seam.** `/data/work` is what the guest's tools write (a
  terminal's working directory and home, projects, a browser's profile,
  scratch), and the rest of `/data` is the guest's own state (ours:
  Hermes' home and the bridge's state). Each is a `DirectoryBackup`
  record of its own, restored together, `/data` first; the work's record
  leaves nothing out. The DO makes `/data/work` with its hold, so it
  always exists to save.
- With `RESTORE_PENDING=1`, the image waits for `/run/computer/restored`
  before it reads `/data`. Without it, `/data` is ready at start (a
  snapshot's wake, or a first start).
- **The restore's check** (optional): an executable
  `/usr/local/bin/computer-check`, run by the DO as root after the restore
  and before it opens the gate, killed past 2 minutes (a failed start). It
  may put back what it copied under the hold. Exit 0: whole. Exit 3: the
  save is unusable, and the DO starts again at once from the save before
  it. Any other: the check failed, a failed start like any.

### Saves and what a wake restores

- A computer keeps its newest three saves (`SAVES_KEPT`), each taken
  under the hold; its view lists them (`saves`), newest first, each
  saying whether its guest answered (`held`).
- **A wake restores the current save**, the newest not found unusable. A
  start whose save will never restore (its archive gone or altered)
  marks it unusable and starts again at once from the one before, with no
  strike against the computer.
- **The snapshot is only a cache** of one save on one image (`{id, image,
  save}`): a wake uses it only when `save` is the current save and
  `image` the pinned image's reference, and one that fails or comes up
  dead is forgotten as the same wake starts the image and the save. So a
  snapshot makes a wake faster, never different.
- **What a wake restored.** Each start that comes up records which save
  it restored and how the life before it ended (the view's `restored`). A
  start that went back in time is a rollback, counted in `rollbacks`: the
  life before it crashed, slept unsaved, or its start fell back past its
  newest save. Nothing's correctness depends on the guest reading either.

### Bounds

Every step the DO takes in a container ends within a bound, and one past
it fails (the Sandbox SDK times out nothing): the consts beside
`SLEEP_STEPS_MS` in cell/src/computer.rs, each within the lifecycle's
deadline for its phase, checked as the cell compiles. So an owner's sleep
answers within 11 minutes.

### The fragment API

- `http://api.fragment.internal/<path>` is the platform's API
  (docs/api.md), and `…/f/<fragment>/<path>` a fragment's own routes
  (`__live` sockets included), as on its host. A storage token minted
  through it names the code.storage the guest can reach.
- `x-fragment-agent: <agent fragment>` says which agent a request acts
  as. The intercept refuses an agent not assigned to this computer, then
  signs the request (NIP-98) as that agent.
- Without the header only the computer's own routes answer:
  - `GET /api/computer`: its own view, proto's `ComputerView` without
    `notices` and with what only the guest is given: each agent's
    `credentials` and `key`, `credentialEnv`, `relay` and
    `relayTargets`. **It changes while the computer runs** (its owner
    assigns or unassigns an agent, connects an account, narrows an agent;
    a deploy changes the models): the guest reads it again while awake
    and runs the new set. The platform never restarts a computer for it,
    and an agent unassigned signs nothing from that moment. `?simplex=1`
    says the image does SimpleX, which its owner's view then says
    (`simplex`).
  - `GET /api/computer/keepalive` (a WebSocket): while it is open the
    computer stays awake. Hold it while work is live (above,
    "Durability"), never while only waiting on a person. A deploy can cut
    the platform's side with no close reaching the guest (#557), and the
    platform then counts it closed, so a guest still busy sends
    `{"type":"ping"}` (answered `{"type":"pong"}`) and opens it again
    when it goes silent.
  - `POST /api/computer/door {agent, door, nonce, key}`: a link's ticket
    redeemed with its door's public key (docs/api.md, "An agent's
    doors").
  - `GET /api/computer/relay?host=&port=`: the XFTP relay, below.
- **A wake subscription**: `{channel, wake: true}` (in place of `url`),
  sent as an agent to `POST /f/<fragment>/api/subscriptions`, wakes the
  computer on each new record from anyone but that computer's own agents.
  Only a computer's egress can ask for one, as a member who may read the
  channel; it goes with the membership. Nothing is pushed into the guest:
  waking, it reads each channel after its last sequence number and
  follows it live. A record's `(fragment, channel, seq)` is the id of
  whatever it starts, so a catch-up never starts something twice (lesson
  2). Each agent subscribes once per channel, after listing its own
  (`GET …/subscriptions`).
- **An agent added to a fragment wakes its computer** (Paul, 2026-10-03:
  to hide a wake's latency): the agent fragment posts `{kind: "joined",
  fragment}` on its `tasks` once for that membership, and the computer
  wakes, so the guest follows the new chat before anyone speaks there.
- What a guest follows, and the records' shapes, are docs/chat-records.md's.
  A record from before the agent's membership's `addedAt` is not for it.

### The XFTP relay

`GET /api/computer/relay?host=&port=` (a WebSocket) carries raw TCP to a
target and back, bytes untouched, because Containers reset a TLS hello
with no server name, which SimpleX's file servers need (`fragment_core::
relay` says why and holds its bounds). Only the targets the view's
`relayTargets` names (`relay`: those on 443, for images that read only
it); past a bound the socket closes with code 4008. The platform sees the
host, the port, the byte counts and the times, never what is carried. Any
process in the computer may use it.

### Connector policy

The deployment's `connectors` (`FRAGMENT_CONNECTORS`;
`fragment_core::connectors::Policy`) name the doors its people may open
and the exact endpoints a computer's relay carries for each: `[{id,
relayTargets: [{host, port}]}]`, each an exact public DNS name on a
nonzero port. Absent, the cell's default: `simplex`, with `relay::HOSTS`
on 443, and `telegram` (a hosted connector, templates/telegram: no
relay). `[]` is the kill switch: no door is linked, redeemed or checked,
and no relay opens; an owner may still unlink. A computer's relay offer
and admission read the same policy, for the connectors its agents are
allowed; the relay's bounds stay the computer's.
`Policy::authorize(owner, agent, connector, action)` is the one decision
every door and relay asks. An image's own declarations grant nothing.

### Models

`http://model.fragment.internal` is the platform's model routes as the
agent (docs/api.md, Models), with the route's bounds and gate:

- `POST /v1/chat/completions`: OpenAI's shape, `model` a tier (`cheap`,
  `medium`; `high` only while it is on, decision 23), `vision`,
  `fallback`, or a model by its id;
- `GET /v1/models`: OpenAI's list of the ids a call may name, answered by
  the intercept itself, naming no agent and metering nothing;
- `POST /v1/audio/transcriptions`: OpenAI's multipart shape, `model`
  `whisper`;
- `POST /v1/decide`: a typed decision (docs/api.md, Decisions);
- `POST /v1/embeddings`: OpenAI's shape, on the deployment's embeddings
  slot.

Any other path is 404, unmetered.

- **Whose call it is** (`fragment_core::models::agent_named`): its
  `x-fragment-agent`, or, from a client that sends no header of its own,
  its key, `Authorization: Bearer agent:<agent fragment>`. Named both
  ways, the two must agree. None, a malformed name, or two that disagree:
  401; an agent not on this computer: 403. The guest's auth headers go no
  further than the computer. The intercept names no fragment, so a call
  bills its agent's owner (decision 36); one its owner's ledger refuses
  gets the ledger's reason (402, 403), and no call is made.
- `x-fragment-tier: cheap` or `medium` (optional) is the caller's tier,
  by which a model's id, `fallback` and `vision` run as its tier's route.
- **A model by its id.** The view's `models` names each slot's model id
  (`ModelIds`), as the deployment names them now. A guest names its
  models by those ids and reads them again while awake: a change of the
  deployment's `models` is a deploy, no new image, and an id no slot runs
  is refused (400). The route's names keep working.
- **Each tier's route.** The view's `models.routes` names each tier's
  label, whether it is private, and its fallback's and vision model's ids.
- **A slot's own fallback.** A call that fails before answering is made
  once more on its slot's fallback, however it named the slot, and never
  a third time. A call that streams nothing is the runtime's own to give
  up on.
- **A slot's `private`.** A private tier is a private route: every slot
  its agents' calls reach (its fallback, its vision, its Deep, and the
  route's four's `fallback`, which an image that reads no route names for
  its stall) is private, or the deploy is refused. Not private yet: voice
  notes (`whisper`), decisions, the runtime's speech, the vendors its
  tools call (web search, `x_search`, image generation), and an owner's
  own model.
- **Fast and Deep** (decision 65). Each tier runs Fast, its own slot, for
  its agents' turns, and Deep for their delegates. `models.routes` carries
  each tier's `reasoning` (Fast's effort) and `deep` (`{id, reasoning?,
  fallback?}`), absent when the deployment names none; an image copies
  them into each profile, and one that reads neither runs delegates on
  Fast. A delegate names its Deep by its id with its tier said, so a
  private child stays on its private slot when Standard runs the same id.
- **Native images** (decision 65). `models.images` lists the model ids
  the deployment declares image-capable; an image gives those models
  attachments and screenshots directly, and has the route's `vision`
  describe them for any other. The route falls back with an image only to
  a model declared capable.
- **A deploy before dispatch** (#625). The model intercept may answer 503
  `deploying` with both `x-fragment-model-not-dispatched: deploying` and
  `x-fragment-release-refused: <release>`: together they promise the
  provider was never called, so the image may send that request again
  within its own bounded wait. Only on this route, and only so marked.
- A call is at most `MODEL_BODY_MAX_BYTES`, charged after its answer
  (decision 62).
- Where each slot runs and its options (body, price, images, private,
  label, fallback, vision, Deep, reasoning, max_tokens) are the
  deployment's `models` (deploy/example.jsonc; decisions 61 and 65).

### Connections and operator keys

Decisions 22, 37 and 44: every credential a guest uses is a per-agent
placeholder in the environment variable its provider's own SDK reads, so
any SDK or CLI works unmodified, with no header of ours. The real
credential is added only at the computer's egress, and only toward the
provider's own hosts.

- **The catalog** is the deployment's `providers` (`FRAGMENT_PROVIDERS`;
  `fragment_core::catalog`): each row a provider, its kind (a
  `connection` through WorkOS Pipes, as the person; an `operator` key,
  metered to the agent's owner at its row's `price`, else the price
  book's; or an `own` key the person gives), its
  hosts, where its credential goes on a request, and its environment
  variables. Adding a provider is a row; no code names one.
  deploy/example.jsonc has the platform's. Connecting one is
  docs/api.md's, Connections; its consent is browser-bound
  (docs/pipes-browser-binding.md), and a connection holds only the scopes
  its person allowed.
- **A placeholder** is `fcx_<provider>_<tag>` for a connection and
  `fck_<provider>_<tag>` for a key, its tag a MAC of (computer, agent
  fragment, provider) under a key only the platform holds (`swap`). It
  names its agent, no one can make one, and it is stable, so an image may
  write it once.
- **The guest learns its credentials** from its view: each agent's
  `credentials`, those it may use now (connected, held by the
  deployment, or given by its owner, and not narrowed from it), and
  `credentialEnv`, every variable of the catalog, held or not, so an image
  can pass them all through once.
- **The intercept** catches the catalog's hosts on HTTPS and HTTP, sends
  on over HTTPS, and swaps each placeholder where its row says it goes
  (`swap::Plan`). It refuses the request, reaching no provider, for a
  placeholder sent to a host not its provider's, in a place its row does
  not name, forged or naming an agent not on this computer now, of a
  provider its agent was narrowed from, or of a connection not connected
  or an own key not given (403 `not_connected`); and an operator key's
  when its owner's gate refuses (402, 403). A body goes as it came, and a
  redirect is never followed. A connection's token is asked of Pipes for
  each request, never held.
- **Who may use what.** An agent may use every provider its owner has
  (decision 44); its owner may narrow it to a list, a specialization
  rather than a wall. Where a connection takes several accounts, each
  agent uses the one its owner picked, else the oldest connected; its
  placeholder is the same.
- Each call a provider answered is counted in the computer's `uses`; an
  operator key's is charged to its agent's owner (docs/ledger.md).

### An agent's own model

A person connects their own model provider (OpenRouter) from Settings
(decision 60; docs/api.md, Connections), and picks its models for an
agent; their account pays.

- The guest is given it as any own key: its placeholder in its variable
  (`OPENROUTER_API_KEY`), the credential's `modelBase` naming its models'
  base URL. Its calls are counted, never charged.
- An agent's `agent.json` `model` (`{provider, id, deep?}`) names its
  Fast model of such a provider, and `reasoning` its effort; `model.deep`
  (`{provider?, id, reasoning?}`) its delegates', inheriting Fast's
  provider and effort when unnamed. It runs on them while its credentials
  hold that provider with a `modelBase`; otherwise on its tier. A Deep it
  cannot use leaves its delegates on its own Fast, never on a platform
  Deep.
- Its owner is shown the same rule: the view's `modelProviders`, per
  agent, are the providers whose models it may run now, and the agent's
  page shows `agent.json`'s model only while its provider is one of them,
  else its tier's model id. An image that does not follow this rule is
  not shown truly.
- The platform holds no runtime code for it: the catalog row says what a
  provider offers, and the view says where its models are.

### Ports

- The image may serve HTTP and WebSockets on any port. The platform
  serves them at the computer's own origin, `<24 hex>--computer.<suffix>`,
  cross-site from the platform, under `/p/<port>/…`, to its owner only for
  now (delegates come with decision 41). Its tickets and cookies are
  docs/api.md's, Computers.
- Only the platform's page may frame a port, so an image's page may not
  frame its own ports either: a screen is one page, its sockets relative
  to it.
- A WebSocket on a port holds the computer awake while open. The
  container may speak first. A close at either end closes the other.
- **The screen** (decision 11) is a page on port 6080, one for each
  agent, at `/p/6080/?agent=<agent fragment>`. The platform carries the
  query and reads nothing in it: which desktop is the agent's, and
  refusing an agent the computer does not run, are the image's. The page
  reaches its sockets by relative URLs, naming the agent again. The shell
  tells it whose screen it is by name, as it loads and on every rename:
  `postMessage({fragment: "screen", agent, name})`; a page takes it only
  from its parent and only for its own agent. Opened in a tab of its own,
  it names none: the computer knows an agent by its label, never its name.

## Our images

- `images/stub`: the bridge with its scripted runtime, the `fragment` CLI
  (its agents ask each other with it), and a screen page. The platform's
  own lanes run against it.
- `images/hermes`: Hermes' desktop image, pinned by digest (its
  Dockerfile names the release), with `hermes-boot`, the bridge as Hermes'
  Relay connector, and the screen.

What our Hermes image guarantees beyond the contract:

- **One profile per agent of Hermes'**, named as its agent fragment,
  holding its `SOUL.md`, `memories/` and `skills/`, synced both ways with
  it. An agent whose `agent.json` names another runtime (Claude) has no
  profile, desktop, route or sync: the boot reads `runtime` as the bridge
  does (`with_runtimes`), and the ready file names it at once, for the
  bridge to run on its runtime. A start reads every agent's before it
  writes or retires a profile, asking again until each answers, so a
  failed read never retires a Hermes agent's profile. One switched to another runtime while
  the computer runs loses its desktop within 10 s, its profile retired at
  the next start; switched back, its profile is written again.
- **Its `agent.json`**, read at boot and every 10 s while awake (a change
  is the next turn's, nothing restarted): `tier` (`cheap` unless it says
  `medium`; `high` only with `FRAGMENT_HIGH_TIER=on`, decision 23),
  `model` and `reasoning` (above, "An agent's own model"), and `home`
  (docs/api.md, "An agent's home"), which its system prompt tells it. Its
  bridge reads `runtime` (docs/bridge.md, "Which runtime runs an agent").
- **Its models by their ids**, from the view: its tier's Fast as its
  profile's model and its Deep as Hermes' `delegation.model`, each with
  its own effort and fallback (none is Fast's chain). A stalled call
  switches to its route's fallback after Hermes' own stale detector and
  one retry; the next turn starts on its tier again. Every call names its
  agent (`x-fragment-agent`, or the key `agent:<agent fragment>` where a
  client takes no header)
  and a tier agent's tier.
- **Its work** is `/data/work/<profile>`: its terminal's working
  directory, its `~`, its temp files and its browser's profile are all
  there, so what its tools write is never Hermes' home. Its Chromium
  keeps its scratch in the container's `/tmp`, in no save.
- **Its hold's answer** (`boot/src/held.rs`): held, it starts nothing new
  of its own, copies every SQLite database outside the work with SQLite's
  online backup, and names exactly those as left out; a database it
  cannot copy (made after the copy, or held locked) is saved hot. Its
  `computer-check` puts the copies back and checks each.
- **What changes under the hold.** Hermes' gateway still writes on its
  own timers while held, each file replaced whole or appended, so a save
  holds a whole version of it; nothing else under `/data` changes (the
  Docker lane's `held_nothing_under_data_changes`). Its model-catalog
  refresh is off, and Node's compile cache is outside `/data`.
- **A turn a restart cut** is closed in its Hermes session before the
  gateway starts, so the next message is a turn of its own (P5).
- **Its agents change while it runs**, nothing restarted: one assigned
  is served within seconds; one unassigned stops at once, its profile
  retired at the next start.
- **Each agent has its own desktop** (Hermes' Bot Desktop), started for
  its screen's first viewer or its first `computer_use` or browser call,
  and stopped after 10 minutes unused. Take over is that desktop's lease,
  which Hermes' tools obey until Give back. Its agents run as one user
  (decision 44), so only the lease and the platform skill keep an agent
  off another's desktop. The desktop and the browser tools share one
  regular Chromium and one saved profile per agent. A link opened on the
  desktop (a terminal's sign-in, `xdg-open`) opens there too: it is the
  desktop's default web browser, so no dialog asks which (FIN-351).

### Approvals in our Hermes image

Nothing technical asks a person (Paul, 2026-10-14): its people are not
technical. No command, file, path or code is put to the agent's owner
(`approvals.mode: off`, and no guard on instruction files), and Hermes'
own rails that ask no one stand. What asks is an outcome, in plain words,
with `clarify` and its choices (`hermes::ASKING`, every turn): before the
agent makes something of its owner's public or shares it with someone
new, sends a message in their name, deletes something of theirs, or
spends their money, unless they asked for exactly that. Hermes' saved
payment card fill still asks. A card is its owner's alone to answer
(decision 42).

Every turn also tells the agent that a file attached in the chat is
already on its computer, at the path the attachment note names, scratch
until it copies it into its work (`hermes::FILES`); and that a reply line
`MEDIA:<absolute path>` attaches a file there (`hermes::SENDING`).

### Its name, in our Hermes image

An agent's name is its agent fragment's title, and its role its
`meta.role` (docs/api.md, "An agent's name and picture"). The image tells
the agent both in its system prompt at every turn (`hermes::told`),
read again every 10 s, so a rename or a new role is its next turn's,
nothing restarted.

### Teammates in our Hermes image

A person's agents on one computer are each other's teammates, on the
platform's records (docs/bridge.md, "Agents asking each other"). Hermes'
own Bot Mode protocol is off in every profile
(`agent.bot_mode_protocol: false`): no roster, no `message_agent`.

- **Each agent is told its teammates** at every turn (`hermes::told_team`):
  each one's name, fragment and what it does (its role, else its job's
  first line), and how asking goes. Read again every 10 s.
- **`fragment_ask`** (`images/hermes/plugins/fragment-ask`, a Hermes
  plugin tool in every profile) asks as `fragment ask --return-to <this
  chat>` does, as the agent whose turn calls it: the agent and chat are
  its session's, never the model's words, and its post id is the tool
  call's, so a retry posts once. It returns at once; the answer comes
  back to the asking chat as a new turn.
- **Each `profile.yaml`** names its agent and what it does, for Hermes'
  own lists, and marks no profile a Bot Mode bot. It is the boot's alone.
- **Each agent is routed to its profile** in the gateway's config
  (`profile_routes`) by one chat, so a session read back from disk has an
  adapter for a background job's completion.

### Skills and the CLI in our Hermes image

- **The managed set** (decision 17) is the owner's skills fragment's
  `skills/` (templates/skills/README.md), installed read-only at each
  start and every ten minutes while awake. No skills fragment, no managed
  skills.
- **The platform skill**, `fragment`, is every profile's: the image's
  CLI's own (`fragment skill`, cli/SKILL.md) after the image's page
  (`images/hermes/boot/src/computer.md`), written at build.
- An agent's own skills win over managed ones, and managed over the
  platform's and Hermes' bundled ones of the same name. A managed skill an
  agent edits is first copied into its own, which then shadows it.
- **The `fragment` CLI** in an agent's terminal acts as that agent
  (cli/GUIDE.md, "As an agent"): it signs nothing; the egress signs.
- **Its credentials** are each agent's placeholders in its profile's
  environment and its terminal's, rewritten within seconds of a change.
  Hermes' own tool backends (web, images, speech) are set only for the
  placeholders an agent holds.
- Hermes' own scheduler is off (decision 38): `hermes cron` refuses,
  pointing at `fragment schedule add --routine`.

### Root in our Hermes image

Paul, 2026-10-05: an agent installs what it needs. Hermes' user has
passwordless `sudo`: the computer is one person's, with no secret in it,
and their agents are not fenced from each other (decisions 13, 43, 44).
An install outside `/data` lasts until the next start from the image (a
crash, an update, a restart; a snapshot's wake keeps it). Nothing is
installed under `/data` at run time but the person's own Claude Code
(below). The bridge runs as root and keeps SimpleX's state from the
agent's user, though not from its sudo.

### Claude Code in our Hermes image

Paul, FIN-233 (2026-10-31): an agent whose `agent.json` names `claude`
runs on Claude Code, which the person installs on their computer
themselves, as on a VPS of theirs. The image ships none, and no flag
gates it.

- **Installed from the chat**: a Claude agent's turn on a computer with
  none asks its person to install it, and Install runs Anthropic's own
  installer as below (docs/bridge.md, "The Claude runtime").
- **Or installed in a terminal of an agent's desktop** with Anthropic's own
  installer, then signed in there with `claude auth login` (its URL
  opened in their browser, its code pasted back). The desktop's home is
  Hermes' (`/data/hermes`), so the install lands in `/data`
  (`.local/bin/claude`, its versions in `.local/share/claude`), already on
  the image's `PATH`, and the sign-in in `/data/hermes/.claude`. Both
  outlast a restart, a sleep and an image update, as all of `/data` does.
- **The agent runs that same Claude Code** (`BRIDGE_CLAUDE_BIN`,
  `BRIDGE_CLAUDE_HOME`: docs/bridge.md), as Hermes' user, with the
  person's sign-in, settings and auto memory, each agent in its own work
  directory. Until it is installed, the agent's turns end saying how.
- **Its updates are Anthropic's**: Claude Code updates itself in the
  background, keeping the two newest versions, so a save carries one or
  two (about 270 MB each).
- **A Claude agent has no Hermes profile or desktop** (above, "One
  profile per agent of Hermes'"): its person installs and signs in from
  another agent's desktop.

## Billing

Awake time, operator keys' calls and model calls are metered to the
computer's owner, and its saves are not (docs/ledger.md, "Who pays").
At zero credit no wake starts, and a computer already awake runs on
until it sleeps (decision 27).
