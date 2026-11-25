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
- Its owner's export holds its view, its newest save that restores, and,
  beside it, the newest save set aside, if it keeps one (docs/api.md,
  Export). It is deleted only with its owner, by an
  operator's wipe (docs/api.md, "Operators: wiping a person"), saves
  included. Cloudflare has no delete for a container snapshot: a wiped
  computer's is forgotten, never restored, and expires within 30 days.

## Lifecycle

Phases: `asleep`, `starting`, `awake`, `sleeping`, and `wont_wake`
(lesson 4). `fragment_core::computer`'s module doc is the whole machine;
an image may rely on this:

- **It is woken** by a record on a channel one of its agents subscribed
  to with `wake: true`, one of its agents added to a fragment, an open
  port tab, or its owner, also as they begin to use it in their shell: a
  chat picked, or the launcher opened by its button or ⌘K (their own
  wake). Never by a page acting on its own (a framed page's ask, or
  anything a page's presence says, typing included: a page sets its
  own), its own guest's traffic, a page loading, a tab shown again, a
  socket reconnecting, or a timer.
- **It stays awake** while the guest holds its keepalive socket, and
  `IDLE_MS` (20 minutes) after what last held it: a keepalive's close, a
  wake, or a port tab's opening, close or use (a message from its page).
  A port tab merely open holds nothing (Paul, 2026-11-18: "an open tab,
  without any action touching the agent, should not keep the agent awake
  indefinitely"). An always-on seat's computer never sleeps (decision 25;
  docs/billing.md).
- **It is saved** when its work ends (its last keepalive or its last
  port tab closes, and 30 s pass with no keepalive opened again), every
  15 minutes while a keepalive stays open or a port tab is in use
  (counted from its first use since the last save), and at every sleep
  (below, "Saves and what a wake restores").
- **A sleep** is, in order: the hold, the save, a container snapshot
  (only after a save that worked), SIGTERM, up to 5 s for the guest to
  exit, destroy. A keepalive opened during an idle sleep's hold cancels
  the sleep. A sleep whose save fails keeps its container and is tried
  again; after `computers.unsaved_max_ms` (30 minutes unless the
  deployment says otherwise) of failed tries it sleeps unsaved, and its
  next wake is a rollback.
- **A wake cancels an idle sleep until its stop begins.** A wake of any
  kind during an idle sleep's hold or save keeps it awake on the
  container it has: nothing is restored or started, and the message
  that woke it runs there. A hold not yet answered is let go at once,
  and the save it was for is due when work settles. A save under way
  goes on as an awake save, holding the guest to its end (a backup cut
  short could be neither kept nor safely forgotten), and is kept or
  failed as one: its failure opens no unsaved window. Its owner's sleep
  or restart is never cancelled, nor a sleep whose stop began: a wake
  then starts it again once it is asleep.
- **A restart** (its owner's) is a sleep that saves if it can and stops
  whether or not it did, then a fresh start: from the image and the
  newest good save, never the snapshot, which could carry what broke it
  outside `/data`. That start is owed, made by its alarm (asleep or
  won't wake, the start alone), so the restart answers `starting` on
  every path, and its owner's page follows it to awake.
- **A stop nothing asked for, while idle, is a sleep.** A host may stop
  a container for its own reasons (its maintenance, a host short of
  memory). One that stops while its computer is awake with no keepalive
  open (a port tab open holds nothing; its use is work), no save under
  way, all this life did in a
  save it made since it came up, and no message or join it was woken for
  that its guest has not taken up, has slept: no strike, no rollback,
  nothing told, and nothing starts it again, though the hold after its
  last work had not run out. Its next wake restores that save. A message
  or a join's notice posted after its guest last opened a keepalive, with
  none open then or since, is work not taken up (a record's wake carries
  when it was posted; a guest at work queues what comes), and a
  life with no save of its own may have been woken for work it has not
  begun, so either stop is a crash (below), as is every other.
- **A crash** (the container stopped on its own) starts it again while
  something wants it; a guest that died holding its keepalive is started
  again however long its turn ran, and its next life ends what was cut.
- **A failed start** (it failed, never reported ready, or its container
  stopped before it was ready) is tried again after a growing pause. It
  is one strike however its failure is heard: a start that fails
  destroys its container, and that container's end is not a crash too.
  The lifecycle keeps that it is destroying (`Phase::Starting`'s
  `destroying`, stored before the destroy), so an isolate that comes up
  after it knows it as well; one that finds the start's own report gone
  with the isolate before it destroys that container (whatever runs: its
  host owns no start yet; an event coming meanwhile waits for it) and
  counts the start failed, once, or, the
  container still running, tells nothing and tries again at the next
  event, an alarm it keeps a minute on at the latest (on its last strike
  nothing else may come).
- **A start held up by the container before it** is no failure. A
  sleep's snapshot the runtime still takes holds its container past the
  sleep's destroy (it takes no signal), so the next start can find that
  container still running. That start started nothing: it is made again
  at once, told as `starting`, no strike, until the container has had a
  stop's deadline (`SLEEP_DEADLINE_MS`, 6 minutes) since the first such
  try; after that each try is a strike, so one that never goes gives up
  as failing starts do. Nothing it would have started from is forgotten.
  A new isolate that finds such a start (it planned nothing yet) never
  takes the running container over as its own: it reports the start held
  up. A port's request meanwhile answers that the computer is still
  starting (409, try again) rather than reach the earlier container.
- **A start's deadline is fixed** (`START_DEADLINE_MS`, its restore's
  share `RESTORE_MS`), never one that follows a restore's progress: a
  restore moves about 40 MB/s (5.7 GB to ready in 147 s on production,
  2026-11-02), so its nine minutes and 25 seconds hold about 22 GB, more
  than a computer's 20 GB disk. Any restore that fits the disk fits the
  deadline; one that does not fit is set aside, not waited for.
- **A start owed after its container goes** (a sleep's end with a
  restart asked or a wake it did not cancel, a crash while wanted, a
  start that fell back past a bad save) is due at once, told as `starting`: the request that saw it go
  (a restart's aside) makes it, and each
  one after it as a start falls back (`STARTS_OWED_MAX` at most), or, in
  an alarm, the alarm it asks for, never the rest of the alarm that ran
  the sleep (see Bounds). Its owner's sleep before it is made lets it go,
  unless the computer is always on.
- **Won't wake**: after `STARTS_FAILED_MAX` failed starts in a row it
  starts no container until its owner asks.
- **Trying again**: "won't wake" is never for good, since what failed may
  pass (a storage error, a disk a release made room on). A wake that is
  not its owner's (a record, a page's presence, a join, a port tab)
  `WONT_WAKE_RETRY_MS` (30 minutes) after it gave up tries one start, and
  a failure gives up again at once: at most one paid start that often,
  and only while something wants it.

## Durability

**The rule** (Paul, 2026-10-05; decision 18 points here): one authority
for each fact. Everything else is a cache, and a cache may be lost or go
back in time without changing what happens.

| Fact | Its one authority | A cache of it |
|---|---|---|
| What was said | the chat's `chat` channel | the runtime's transcript |
| Which turns started, which calls they asked for, what they did, how they ended, each card's answer | the chat's `work` channel, the journal (docs/chat-records.md) | the bridge's state (`/data/bridge`) |
| The agent's self (`SOUL.md`, memories, skills) | the agent fragment's repo | the profile's checkout |
| The runtime's context and the agent's working files | `/data`, as of its newest save | the snapshot, for one image |
| Which save is current, and what a wake restored | the Computer DO | |
| Usage | the ledger | |
| A messenger's encryption state | SimpleX's core's databases in `/data`, as of the newest save (docs/bridge.md, "SimpleX") | |

A guest holds its keepalive only for live work: a turn queued or
running, delegated work still running after its parent replied
(decision 65; docs/bridge.md, "Runtime work outside a turn"), or work
left running while the computer is working, up to 2 hours after the last
of those (docs/bridge.md, "Work left running"). Waiting on a person
never holds it, but for a card only this life can take, up to 10 minutes
(docs/bridge.md, "An answer for the computer"). That work is never saved: a restarted runtime has none
of it.

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
  card outlives its turn (P6)"). Proved on real Hermes by the `hermes`
  lane: a turn cut under its card by a sleep and a wake, then its late
  answer. Not yet by a hosted run's real sleep and wake.

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
  Deferred (Paul, 2026-10-05: "too much of a lift for now"); being built
  since 2026-11-20, so no one needs an always-on computer for SimpleX. A
  SimpleX door: a `lite` container per linked account, SimpleX's core and
  the door's runner (images/simplex-door) with the bridge's own link
  rules, its databases saved after every change as one file its door
  keeps; a save its door knows is behind (an effect it numbered is newer)
  forces one ratchet sync before anything is sent. The runner's protocol
  with its door is images/simplex-door/src/door.rs; its Durable Object is
  cell/door.mjs (each save a file in the bucket under `doors/<door>/`, a
  row naming it, the newest three readable ones kept; a save that will
  not restore is never given again, and a door with saves never starts as
  a new one over them). A deployment runs doors only when its config names
  their image (`doors`), and a local stack only for the e2e's
  `simplex-door` lane (with `simplex-link`, its doors reach SimpleX's
  servers for a real link). Its connector (templates/simplex, a hosted door as
  Telegram's is) starts it and is its only caller of the two: its jobs
  reach the door of the connector's life (`door_id`: a name made again
  reaches another), and the door posts its
  owner's messages to the connector's inbox as a webhook does, which
  publishes each to the bound agent; the agent's computer wakes for it as
  for any message, so it may sleep. It carries files both ways (S1): its
  owner's (a voice message, a photo) received by its core and given to the
  door, which holds each a moment under `doors/<door>/files/` until its
  connector keeps it as one of the chat's blobs; a reply's, which the bound
  agent uploads to the connector's chat (a viewer there, it may upload its
  replies' files), given to the door by its connector and sent after the
  reply's text; its core's XFTP traffic through the door's relay (below).
  Settings makes one for every seat,
  its setup page showing the invitation's QR code (D3); an always-on
  computer that does SimpleX keeps each agent's own (decision 32) until
  existing links move to doors.

## What its owner is told

Paul, 2026-10-08: no one's computer may die unexplained while they use
it, and each needs a way back to working.

The owner's view carries `notices` (proto's `ComputerNotice`, from
`fragment_core::computer::notices`; their fields are docs/api.md's), the
most pressing first. Each is derived from the Computer DO's own state
with no clock, so every reader is told the same, and when they change the
DO tells its owner's open pages (`/api/fragments/watch`'s `changed`), as
it does when its phase changes (`Lifecycle::shown`: a chat with their
agent says it is getting ready while it starts, before the start's own
steps; its "no reply yet" waits until it is up):

- `wont_wake`: its starts kept failing.
- `disk_filling`: its room (below, "Room to restore": what a restore of
  its save would leave, never what `df` says), read in each start that
  restores a save or a snapshot, before its guest is let go (`df`, a step
  of the start, within its deadline), and after each awake save (`df` and
  `du` of `/data`), is at most `ROOM_TOLD_MAX` (3 GB): the room left,
  told until a read finds at least `ROOM_CLEAR_MIN` (4 GB), so a disk at
  the edge never shows and clears it in turn. At no room it says its
  computer may not start again until there is room. Each read logs a
  `disk` line of its own, its room in it. It tells its owner to ask their
  agent to move big files to storage: the agent's `disk-space-finite`
  skill makes room with `fragment offload`, which removes a file only once
  its copy in a members-only fragment of the owner's reads back as it was
  (cli/GUIDE.md, "Moving files off a disk"). Both views carry the read
  and its room (`disk`), the guest's for its agents: each agent is told
  firmly at its next turn after each read that finds it filling
  (docs/bridge.md, "An agent is told to make room").
- `unsaved`: this life's saves are failing (a sleep's at once, awake ones
  from the second in a row): since when its work is in no save, and when
  the bound stops it.
- `went_back`: a start went back to an older save, or will at its next
  start, told until its owner says they saw it. None when nothing was
  done on the lost life after that save (Paul, 2026-10-23). When nothing
  that may change its files ran after it (no turn, no port tab used:
  `unchanged`), it says where it went back to and no loss. When its
  newest save didn't fit its disk (`tooBig`), it says so, where it went
  back to, and that the newer files are kept and can still be exported.
- `update`: it is awake, with no keepalive open, on another version than
  its next start runs, until its owner puts that version off. While its
  save may not restore beside the new image, it says to make room first,
  with no Restart (`roomFirst`; below, "An image's share of the disk").

The platform posts none of them in a chat: a chat's records are its
people's and agents' (docs/chat-records.md). Its owner reads them in words
with the rest of what they are told (`GET /api/notices`, docs/api.md,
Notices): the computer says each one itself (`fragment_core::computer::said`),
with its actions' routes (Restart, OK `…/notices/seen`, Later
`…/notices/later`, Get help), and the shell
draws them over every chat and Settings with no code of its own for any.

- **One way back: Restart** (`POST /api/computers/{id}/restart`): the
  sleep and fresh start above. It names the start its owner saw, so
  pressed twice it restarts once, and the ledger is asked first, as for a
  wake. While it runs, the shell shows its phase where the button was
  (saving, then starting; Settings' State says Saving, not Going to sleep,
  while a restart asked in that page saves: one asked elsewhere, by
  another tab, the CLI or the app, still reads Going to sleep there), read
  every two seconds while the page is shown
  (`cell/shell/restart-follow.js`), and a line that a big save or restore
  can take a few minutes once it has run 30 seconds; awake on its new
  start (or, pressed while a start was under way, which the restart
  adopts, on that start, read after its answer), the notice goes. A read that fails says so, with Check again,
  never that the restart failed; an ask that fails is decided by the next
  read that works (none while the page is hidden or waits for Check
  again): no restart under way says it didn't restart, with Try again.
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
- **The agent** is told what a cut lost, and the chat when a rollback or
  a loss of `/data`, or of its runtime's memory alone, made its runtime
  forget it, in its next turn in each chat (docs/bridge.md, "The turn
  after a cut one is told" and "What a runtime forgot").
- **Getting help**: a deployment may name `support_url`, which a
  `wont_wake` notice offers beside Restart.

## Preview and production

**There's preview and there's prod** (Paul, 2026-10-31). Two images, each
by its name (xtask/src/image.rs, `IMAGES`): the computers' `hermes`, and
the SimpleX doors' `door`. Preview runs master's of each, and production
runs the digests preview proved, promoted by Paul.

- **Pins are digests.** Every image a CI config names is pinned at
  `…@sha256:<64 hex>` (deploy.rs's `pinned_ok`); tags only find them. A
  computer is pinned to an image by its name in the config (`hermes` by
  default); the doors' image is the config's `doors.image`. A computer
  pinned to a name the deployment no longer has has no image to start:
  move its pins (`fragment operator pin`) before a name goes.
- **Preview follows master.** Each green master push deploys, as
  `hermes` and as the doors' image, the images its checkout's inputs make,
  each reused by its own input key when it was built before
  (`xtask/src/image/preview.rs`), in its own preview repository
  (`fragment-preview-computer-hermes-canary`, `fragment-preview-door`),
  else built beside the push's e2e by master's job, the only writer of an
  input tag. The deploy pins them in memory (`--preview-image
  hermes=sha256:… --preview-image door=sha256:…`) over what preview's
  config pins; a config that names no doors runs none.
- **What changes a version.** Only an image's own inputs (Paul,
  2026-10-14: a change to the cell or the shell needs no image rollout):
  the door's runner moves the door alone, the CLI Hermes alone, and the
  bridge both; a host test holds each list to its Dockerfile.
- **Production stays pinned** (`production.jsonc` in
  finitecomputer/fragment-deploy) to the digests a round proved, each
  copied byte for byte into production's registry (`cargo xtask image
  copy`: `fragment-computer-hermes`, `fragment-door`), never rebuilt.
  Only Paul promotes.
- **Guest-facing contracts stay additive.** The cell can be ahead of a
  computer for days (an awake computer keeps its image across a deploy),
  and production's image lags master by a round.
- **A Hermes release is forward only** (Paul, 2026-10-13). A change only
  to our layers on the same Hermes base can be pinned back. A new Hermes
  release may advance its databases' schemas, and a computer's three
  saves soon hold none from before it, so it is fixed forward
  (docs/technical-debt-ledger.md, "A Hermes release has no rollback").

### One round before production

1. Record each image's input key and digest from master's preview deploy
   summary, and the running reference of each computer watched (a later
   push may move preview).
2. Live on it: an old chat continues, a new one recalls a memory, skills,
   a routine, the screen and Take over, Google, a teammate's ask answered
   back. Watch `version == latest`, `rollbacks` unchanged, every save
   `held`, no `wont_wake`, `unsaved` or `went_back`, operator health,
   logs and spend.
3. Run the hosted lanes on preview, then `--sweep`: `cargo xtask e2e
   --hosted --config <preview config> --branch preview --only
   computers,agent-restart,agent-smoke,agent-ask
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
6. Paul promotes it: the Image workflow's production copy of each image
   (`from` its proven reference; the door's once production runs doors),
   each pin committed to `production.jsonc` (Hermes's old line kept for
   rollback), then a move of `production`.

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
| `FRAGMENT_DISK` | `fpac` when it mounts `/data` from its disk (below, "A disk") |

The guest holds no credential. Every request to those hosts, and to a
provider's host (below, "Connections and operator keys"), goes through
the Computer DO's intercepts, which add what the platform holds; other
traffic goes out as it is (decision 43). Each start registers them
before the container starts, at once, so the guest's first request never
races them (`cell/intercepts.mjs`): the platform's hosts and the catalog's
providers', asking nothing. Its agents' Vault secrets' hosts are asked of
its owner's Vault: beside the container's start when the start holds its
guest until it lets it go (a restore's gate; a snapshot's hold, when its
save was held), and caught before that; before the start when its guest
runs at once (a fresh start, or a snapshot whose sleep's hold failed), so
no guest runs with one uncaught. HTTPS to an intercepted host is
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
  before the signal; whatever is written after it may be lost. On a disk
  it ends with its disk's final flush, which the DO waits for
  (`DISK_EXIT_WAIT_MS`, 150 s; below, "A disk").

### What every image carries

- `/usr/local/bin/sandbox-shim` from `cloudflare/sandbox:1.0.0`: the DO's
  `DirectoryBackup` saves and restores `/data` through it.
- `sh` (with its `test` and `printf`), `true`, `mkdir`, `touch`, `rm` and
  `cat`, with which the DO polls, marks and reads its markers; `df`, `du`
  and `cut`, with which it reads its disk (below, "Room to restore"). GNU's
  and BusyBox's alike.

### The hold

A handshake before every save. The DO removes `/run/computer/held`,
touches `/run/computer/hold`, and waits up to `HOLD_WAIT_MS` (20 s) for
the guest to touch `/run/computer/held`: its answer that it claims
nothing now (no claim in flight, none new while the hold exists) and has
copied what a hot copy would tear. Then the DO saves. An awake save lets
go (it removes the files) once its home's record is taken, the rest of
`/data` with the copies in it, and takes its work's while its guest
works: the hold never made the work whole (what the guest already claimed
writes to it during any save, and nothing in it is copied under the
hold). A sleep keeps the hold until its container is gone. A guest that
never answers is saved anyway, the save recorded as not `held`. What the
guest already claimed may run on. A fresh container never has the files:
the DO removes them from one started from a snapshot.

- **What the save leaves out.** `held` may name, one per line, what the
  save of `/data` leaves out: gitignore patterns relative to `/data`, the
  files the guest copied under the hold to names the save keeps, so a hot
  copy of one is never what a wake restores, and caches a tool fetches
  again. Those under `/work/` its work's own save leaves out, relative to
  it (`fragment_core::computer::work_left_out`). Bounds and the patterns'
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
  leaves out only the caches the guest named in it. The DO makes
  `/data/work` with its hold, so it
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

### A disk

A computer made while its image is one its deployment puts on disks
(`computers.disks`, `FRAGMENT_COMPUTER_DISKS`; preview's setting,
production's off) is on a disk for good: its `/data` is ext4 on a lazy
block device, FPAC (github.com/finitecomputer/fpac, at the v0.1.0 our
images pin by its SHA-256), whose bytes are objects in the platform's
bucket under `disks/<its id's hex>/`. Only new computers: one with saves
never goes on a disk. Its pin refuses an image the deployment does not
put on disks.

- **What it keeps.** Its Computer DO keeps the disk's head (Computer
  step 10, `disk`): the newest checkpoint its daemon posted, whether that
  life ended clean, and the head before its newest life's, the one a
  start falls back to. The head moves only by
  `fragment_core::computer::fpac::decide`, FPAC's own rules, and only
  from the life that runs. Everything else is files: objects named by
  life, put once, never overwritten.
- **Its route.** Each start registers `fpac.fragment.internal` beside
  the platform's hosts (`cell/fpac.mjs`), by FPAC's contract: `GET
  /open` names the disk, that life's epoch (its generation), its volume
  (20,000 MiB) and its head; `GET` and `PUT /o/<name>` read and put
  objects (bodies streamed, a put checked against its SHA-256 and refused
  over an existing name, only under its own epoch); `POST /head` goes to
  the DO's rules. Beside the contract, the platform's own `POST /unusable
  {why}`: its boot's word that the head it was handed will not open,
  which the route sends on with that life's epoch and that head. The
  guest names no disk and holds no credential.
- **Its start.** No restore, no gate, no snapshot: `FRAGMENT_DISK=fpac`,
  and the image's boot mounts `/data` before anything reads it, its
  daemon's log and status on a tmpfs at `/run/fpac` (never on the disk
  its cache fills). The DO waits for its guest's first read of its view
  (`DISK_ALIVE_MS`, 2 minutes): that read says what its mount did. A head
  that will not open, its boot says (`POST /unusable` on its disk's
  route) and then waits to be destroyed; the DO takes it from the life
  that runs alone, the start fails as `unusable`, and, while the head
  stored is still the one that start was handed, the head falls back to
  the one before it, and it starts again at once, no strike (that life's
  writes since lost, and logged). With none before it, a strike as any
  failed start; with the head moved since, it starts again on that one. A
  new disk's first life that ended before anything of it shipped (its head
  a claim, nothing to replay: a blank device) starts again as a new disk.
- **Its sleep is a signal and a wait.** Its port tabs close; the DO
  measures its `/data` for its owner's Storage (the `df` and the walk an
  awake save's measure makes, of its own filesystem); it reads its
  daemon's status (`fpac status`) until nothing is unshipped, at most
  `DISK_DRAIN_MS` (45 s), and a wake meanwhile cancels the sleep
  as a hold's wake does; then SIGTERM, its image's boot stops its own
  programs and its daemon's final flush ships what is left and posts a
  clean head; the DO waits for the container to exit
  (`DISK_EXIT_WAIT_MS`), destroys it, and logs its sleep's line. A crash
  loses at most what its daemon had not shipped, about a second of
  synced writes: the next mount replays the life's segments past its
  last checkpoint.
- **No saves of its own.** A disk is saved only for its owner's export:
  the export asks its computer for a save taken after it began, which is
  taken awake under the hold as any awake save (waking it, so an export
  costs its awake time; one it may not wake has none to give), and looks
  again every 20 s for at most 20 minutes from its first look, then goes on
  without it, saying so. The ask lasts the life it was made in; the export
  asks again while it waits. On a disk, Hermes' boot never puts a hold's
  copies back: the live databases are newer. Its view lists those
  saves; no start restores one; `restored.from` is `disk`.
- **What an image does.** It carries `/usr/local/bin/fpac` and `mke2fs`
  1.47 or later (a new disk's ext4), and on `FRAGMENT_DISK=fpac` mounts
  `/data` with it, waits for the mount, says what it did with its first
  read of its view, and on SIGTERM waits for its daemon's final flush
  before it exits (`fragment_bridge::disk`: our Hermes image's boot, and
  the stub's bridge as its PID 1). The runtime gives its container FUSE
  and a loop device: Cloudflare's does, and krun's on a self-hosted box;
  workerd's Docker does not.
- **A wipe** deletes its disk's objects with its saves'.

### Saves and what a wake restores

- A computer keeps its newest three saves (`SAVES_KEPT`), and the one its
  running life came up from until a start comes up from a newer one (a
  save a start proved, the last a start falls back to), each taken
  under the hold; its view lists them (`saves`), newest first, each
  saying whether its guest answered (`held`).
- **A wake restores the current save**, the newest not set aside nor found
  unusable. A
  start whose save will never restore (its archive gone or altered)
  marks it unusable and starts again at once from the one before, with no
  strike against the computer. A read of its archive that storage failed
  (R2 answered an error) is asked again a few times within the restore
  (`cell/backup-reads.mjs`), since the SDK's shim would fail the whole
  start on one.
- **A save too big for its disk is set aside** (Paul, 2026-11-20:
  "rollback is preferred to being stuck"). A restore can need a little
  more room than the life that saved it used: the sandbox runtime stops
  one that would leave less than its floor free (2026-11-20: a computer
  whose agent filled its 20 GB disk saved, slept, and never started
  again, each start "restore stopped with ~203 MB free, below the
  268435456 byte floor"). A restore that found no room
  (`fragment_core::computer::restore_found_no_room`: the runtime's guard
  in its own words, or the disk filling, `ENOSPC`, which the cell's host
  names a restore's `SandboxFileError` by, and nothing else) sets its
  save aside (`too_big`), as an operator's choice of the save before it
  would (below), and starts again at once from that one, with no strike.
  With no save before it, nothing is set aside: the failure is a strike
  as any, so a computer none of whose saves fit ends `wont_wake`, never
  started with nothing. Its owner is told it went back because its
  newest save didn't fit its disk, where to, and that the newer files
  are kept and can still be exported (`went_back`'s `tooBig`).
- **An operator's start from a save** (`fragment operator start-from`;
  docs/api.md, Operators) is the way back when a save restores but fails
  every start for some other reason. While it runs no
  container, an operator names a save it keeps; its next start restores
  it, and each newer save is **set aside** (`setAside`, when), and so is
  each older one past the three newest before it, so a start's fallbacks
  reach every save it may restore. One set
  aside is kept 30 days (`SET_ASIDE_KEPT_MS`) from then, outside the
  three kept, then let go of at the next save or choice; at most six are
  (`SET_ASIDE_MAX`, two choices' worth: past it the oldest go). They are
  kept apart from the saves a start may restore, so a release from
  before them restores the save chosen and lets none of them go (it
  forgets them: their archives stay in R2 until a wipe). Its
  owner's export copies the newest save it may restore and, beside it,
  the newest set aside (`set-aside/`: its files may be newer), however
  many saves it took since; no start restores one, whatever fails after. A later choice may name one set
  aside, which is then restored; what an earlier choice set aside stays
  so for its time. One that won't wake is asleep after the choice, its
  strikes counted anew, so its next wake of any kind, or a restart,
  starts it; the start that comes up went back in time, and its owner is
  told so (`went_back`, naming the save it went back to).
- **The snapshot is only a cache** of one save on one image (`{id, image,
  save}`): a wake uses it only when `save` is the current save and
  `image` the pinned image's reference (or, while its update waits for
  room, the image it ran: below, "An image's share of the disk"), and one
  that fails or comes up dead is forgotten as the same wake starts the
  image and the save. So a snapshot makes a wake faster, never different.
- **What a wake restored.** Each start that comes up records which save
  it restored and how the life before it ended (the view's `restored`). A
  start that went back in time is a rollback, counted in `rollbacks`: the
  life before it crashed, slept unsaved, or its start fell back past its
  newest save. Nothing's correctness depends on the guest reading either.

### Room to restore

A save is only good while a wake can restore it, and a restore takes more
of the disk than the running computer did (2026-11-20: a computer's 20 GB
disk ran with 2.1 GB free and saved 9.5 GB, and every start after it
stopped in its restore of `/data/work` with about 200 MB free).

- **Why.** A computer's disk compresses what is written to it: it is
  btrfs with zstd, by every sign (`df` reads exactly the device's
  20,000,002,048 bytes, which ext4 and XFS never do, and an empty
  computer's image takes about 2.6 GB of it, though its files are 4.3 GB).
  A restore writes each file whole, uncompressed: the Sandbox SDK's
  `sandbox-shim` 1.0.0 turns compression off for what it extracts, for
  speed (`FS_NOCOMP_FL`, `Extractor::new`), and its files keep that flag.
  It also writes a sparse file's holes, and a file hard-linked between
  `/data` and `/data/work` once in each record. It stops once less than
  its floor is free, a twentieth of the disk and at most 256 MiB
  (`restore_floor`), checked every 8 MiB it writes. It stages nothing: it
  streams the archive, extracts beside its directory and swaps it in, and
  a fresh container's `/data` is empty.
- **So a save restores** while `IMAGE_DISK_MAX` + what its records hold
  at full size + `restore_floor(total)` fits the disk. `IMAGE_DISK_MAX`
  (3.0 GB) is the most of a disk an image takes as a fresh computer reads
  it: an empty 20 GB btrfs disk keeps 0.55 GB for itself, and our image
  took 2.41 GB more (production's empty computers read about 2.6 GB used;
  the larger is counted).
- **What a restore writes** is read after each awake save, in the alarm
  that saved it (`sizes_read`, within `SIZES_READ_MS`, 30 s): `du` of
  each record as its save takes it, `/data` but its work less what its
  held guest named, and `/data/work` less what it named under its work,
  each the larger of its allocated and its apparent size, summed. Apart,
  a file hard-linked between the two counts in each, as each record
  restores its copy; GNU's `du --exclude` leaves out what the save does
  without walking it, so a file hard-linked between a cache left out and
  the work kept (uv's and pnpm's caches link into projects) counts where
  it restores. The shell finds what each pattern names (a `*` never
  crosses a `/` nor matches a leading `.`, as the save's gitignore-style
  patterns' `*` is one name), so `du` leaves out those paths alone, and
  where the two differ it counts more, never less; each pattern is in the
  command once, so a guest's largest answer fits one exec's argument
  (`EXEC_ARG_MAX_BYTES`). A `du` with no `--exclude` (BusyBox's, the stub's) counts
  all of `/data`, once. A record not read leaves the last read's size in
  place, and a walk that failed part-way still gives its `df`. On ext4
  the allocated size is exactly what a restore writes; on btrfs it is a
  little more (a small file sits in its metadata), and a big file's
  restore writes a little more than it, so 1% more is counted
  (`restore_writes`). A start's read is `df` alone and keeps the last
  `du`, unless that start restores another save (one set aside as too big
  for its disk, or an operator's choice): the size was the save's it
  left, so it is forgotten, and the start's `df`, after its restore, says
  the room.
- **What takes the disk** is read with it (`sizes_of`): the folders of
  all of `/data` (its caches too), to `SIZES_DEPTH` (4) below it, and its
  files of at least 10 MiB anywhere, each the biggest first, at most 64
  and 32, a path at most 400 bytes, 16 KiB of paths in all, and `/data`'s
  own total. It comes after the records and the `df` (read first, so a
  line it lists can never pass for theirs), its walks ending within
  `SIZES_LISTED_BY_S` (24 s) of the read's start, so it never costs them
  their read; each line framed by GNU's `du -0` with a newline in a name
  read as `?` (a name can forge no line; BusyBox's, the stub's, frames
  none, so its total is its `all` record), and the whole answer within
  what the cell keeps of it. A read it cannot use logs its exit, its
  length and the exec's error code alone. Kept in a row of
  its own (`sizes`), shown on its owner's view (`sizes`) and never on its
  guest's or an operator's, never logged: it names a person's files. Its
  view adds its folders by kind (`kinds`, `storage_kinds`): its agents'
  own (all of `/data` but its work), and in its work caches, browser data
  and downloads by their folders' names, projects the rest.
- **Settings → Computer → Storage** (cell/shell/computer.js) draws it
  from its owner's view, so asleep, awake and not waking alike: a bar of
  what takes its disk by kind, the room left (red while it is told
  filling, "Full" at none), its five biggest files by name, and when it
  was measured; before its first read, that it is measured after its next
  save. A view that lands while Settings is open redraws it.
- **Delete** (`POST /api/computers/{id}/files/delete`), on an awake
  computer, asked once in Storage: the platform removes one regular file
  of its work in its container, no agent asked (so it works when its
  agent is stuck), then reads its disk again. Never a folder, a link, or
  a path outside `/data/work`: the path is held to `work/…` with no `.` or
  `..` part (`delete_read`), and the command it runs (as root) resolves it
  and holds it to `/data/work/` again, so a link to a folder elsewhere
  leads nowhere. A path with a `?` is refused, and Storage offers no
  Delete for one: GNU's listing reads a newline in a name as `?`, so it
  may name another file. Its line logs whether it deleted, never the path,
  and a delete its computer did not finish says so, never the path
  either; Storage says "Deleted" when its read after it did not answer in
  time. A process of the computer's own that swaps a folder for a link
  between the check and the `rm` could lead it out of the work: the
  agent is root in its container already, and the check is against the
  platform's mistakes, not its agent.
- **Its room** (`fragment_core::computer::room`) is the least of what is
  free now and what a restore of that `/data` leaves beside
  `IMAGE_DISK_MAX`, each less the floor. Told at `ROOM_TOLD_MAX` (above,
  `disk_filling`).

Measured on 20 GB btrfs disks with zstd (as this box's Docker's, and by
its `df` a computer's), each class of files written as a computer writes
them, saved and restored with the real `sandbox-shim` 1.0.0
(2026-11-20; the bench, its numbers and the raw results are in the PR
that added this):

| Files | Running | Restored | × |
| --- | --- | --- | --- |
| Our Hermes image's (4.3 GB, 121,568 files) | 2.41 GB | 4.11 GB | 1.70 |
| A Python environment (Hermes', 90,716 files) | 1.07 GB | 2.12 GB | 1.98 |
| Source code (12,300 files) | 113 MB | 285 MB | 2.53 |
| `node_modules` (4,984 files) | 197 MB | 449 MB | 2.27 |
| Git packs, video, a browser's SQLite history | 1.41 GB | 1.42 GB | 1.01 |
| 100,000 small JSON files | none (in metadata) | 132 MB | |
| A 1 GiB sparse file, 1 MiB written | 1 MB | 1.08 GB | |
| 200 MB hard-linked between `/data` and its work | 210 MB | 423 MB | 2.01 |

The same files on btrfs without compression, and on ext4, restored to
what they used running (1.00), but for the sparse file and the hard link.

The incident, made again on such a disk (our image, two Python
environments and 13 GB of video, until `df` read 2.1 GB free): it saved
13.9 GB, and its restore onto a fresh disk with the image stopped as
production's did, "restore stopped with 261689344 bytes free, below the
268435456 byte floor". With the image's own share measured, `du` (16.8
GB) predicted 36 MB of room: 43 MB, 0.26%, short. Counted as
`room` counts it, it had none; it is told at 3 GB of room, which that
disk reached with 3.2 GB less video, `df` reading about 5.3 GB free.

### An image's share of the disk

Paul, 2026-11-20: "our image update either never goes up in size, or
that it's safe for all existing users somehow". An image sits on the same
disk as `/data`, so an image that grew would leave every computer less
room to restore, and a save that fit beside the old image might not fit
beside the new one.

- **A budget.** Every image is held to `IMAGE_DISK_MAX` (3.0 GB), the
  share of a computer's disk it may take as a fresh computer reads it: its
  files written to a fresh btrfs disk of a computer's size (`DISK_MB`)
  with zstd, less what that disk has available. One measure does it
  (`cargo xtask image disk <reference>`, xtask/src/image/disk.rs, reading
  both numbers from their one definition in core), wherever an image is
  held to it: a push (`cargo xtask image push`, the Image workflow) and
  preview's build (`image preview --build`, CI's preview image) are
  refused past it before anything pins them, and the Docker lane measures
  the Hermes image on every change to it
  (`the_hermes_image_fits_its_disk_budget`). Preview's build is pushed as
  `unchecked-<key>` and named by its inputs (`inputs-<key>`, the tag a
  later run reuses) only once it fits, so a retry after a refusal builds
  and measures again. Today's
  image measured 2.53 to 2.67 GB in five runs on a quiet box (2026-11-20;
  production's empty computers read about 2.6 GB used), and 2.99 GB once
  under heavy disk load, when btrfs compresses less as it writes back in a
  hurry: the budget is that plus a little. Raising it is Paul's word alone, recorded
  beside the constant: each GB raised is a GB less of every person's files
  that restore.
- **Room counts the largest image.** A computer's room (above, "Room to
  restore") counts `IMAGE_DISK_MAX`, never the image it runs, so it is
  told to make room while its save still restores beside any image the
  deployment may move it to.
- **An update waits for room.** A computer whose save may not restore
  beside the largest image (`fits_any_image`, its disk as last read), or
  whose disk is told filling (at the edge, a save a little bigger no
  longer fits), stays on the image it has: a wake starts its snapshot of
  that save on the image it was taken of, though another is pinned
  (`Saves::update_waits`). Its `update` notice, told as ever, says to make
  room first and offers no Restart, which would take the new image, and
  so does Settings (its view's `roomFirst`). Its owner's restart stays on
  its image too: it keeps the snapshot, so the restart's sleep takes one
  and its start comes up from it, not fresh from the pinned image. Once a
  read after a save finds room (and it is told filling no more), its next
  wake takes the new image, and a restart is fresh again. A start with no
  such snapshot (after a crash, or snapshots off) has no older image to
  stay on; there the budget is what keeps it restorable.

### The start line

The Computer DO logs one line per start (`fragment_core::computer::start_line`),
for Workers Logs and `wrangler tail` to count which path wakes take and how
long each step is. It begins `{"start":{"generation":`, names the computer
by its id alone, and is at most 512 bytes:

```json
{"start":{"generation":7,"end":"ready","woke":"record","path":"restore","why":"crash","bytes":1480000000,"newImage":false,"beganMs":40,"upMs":900,"dataMs":37000,"readyMs":37050,"heardMs":37800,"turnMs":39100},"computer":"computer:17c33cdf88c6d1d8f58b7a0f"}
```

It is written once, as the start ends: `failed`; `replaced` (a later start
began before its end was heard); or `ready`, once its guest opens its first
keepalive (a turn), `TURN_WAIT_MS` (a minute) passes after it came up (it
is written at the next event or view read: the guest reads its view each
minute), or its life ends, whichever is first.

- `failed`: `start` (its own steps), `snapshot` (the snapshot would not
  start), `dead` (its guest never read its view within its wait: a
  snapshot that came up dead, or the `alive-check` lever),
  `unusable` (its save would not restore), `no_room` (its save's restore
  found no room on its disk: set aside), `exit` (its container stopped
  first), `deadline`, `blocked` (the container before it still ran: it
  started nothing, and the start made again counts from the same wake),
  `other`.
- `woke`: what asked for the wake it answers: `record`, `tab`, `owner`,
  `joined`, `restart`; `itself` when nothing did (a start after a
  crash, an always-on start, or an ask an earlier isolate heard). A wake's
  fallback and retries carry its first ask.
- `path`: `snapshot`, `restore` (the image, then the save whole), `fresh`
  (no save yet), `disk` (its disk mounted: "A disk").
- `why`, for a restore, the first that holds: `off` (no snapshots here),
  `restart`, `aside` (its newer saves were set aside: an operator's
  choice, or too big for its disk), `unusable`
  (a newer save would not restore), `image` (the
  snapshot is of another image), `dead_snapshot` (this wake's snapshot
  start failed), `crash`, `unsaved` (its last sleep could not save),
  `no_snapshot` (its last life ended saved and no snapshot was kept: a host
  stopped it idle, or that sleep's snapshot failed), `other`.
- `bytes`: the stored, compressed size of the save it restores, or that
  its snapshot caches.
- `newImage`: it runs another image than its last life did (absent before
  its first life). Whether the host pulled it is not known to the DO.
- Times, in ms from the wake asked (or the start's own beginning when
  nothing asked): `beganMs` the start made (a sleep under way delays it),
  `upMs` its container took an exec (snapshot and restore only), `dataMs`
  the restore's gate opened, `readyMs` it came up, `heardMs` its guest
  first read its view (our image's boot reads it first, then its bridge),
  `turnMs` its first keepalive (its bridge has a turn queued or running).
  When its bridge follows its chats is not visible to the DO, so it is
  left out.

- `disk`, a start on a disk: `mountMs` (its daemon's start to its
  mount, as its boot saw it), `misses` (extents a read fetched by its
  first view read), `refetches` (its fetches made again: a checksum that
  did not match, or the remote unavailable), and `sleptUnshipped` (the
  bytes its last sleep's wait left for the final flush), each left out
  when unknown. A start whose head would not open fails `unusable`.

The line is in memory until it is written: a deploy or a crash of the
platform during a start, or before its first turn, loses that start's line.

### The sleep line

A disk's sleep logs one line too (`start_line::sleep_line`), as its stop
ends, at most 512 bytes:

```json
{"sleep":{"generation":7,"unshipped":0,"misses":41,"refetches":0,"drainMs":180,"stopMs":2900,"exited":true,"clean":true},"computer":"computer:17c33cdf88c6d1d8f58b7a0f"}
```

`unshipped`, `misses` and `refetches` are its daemon's as its wait last
read them (left out when unread); `drainMs` the wait, `stopMs` the
signal to the container gone; `exited`, its guest exited within
`DISK_EXIT_WAIT_MS`; `clean`, its head after it is that life's and marks
a clean sleep (the next mount replays nothing).

### Bounds

Every step the DO takes in a container ends within a bound, and one past
it fails (the Sandbox SDK times out nothing): the consts beside
`SLEEP_STEPS_MS` in cell/src/computer.rs, each within the lifecycle's
deadline for its phase, checked as the cell compiles. A save's steps run
in the Durable Object alarm whose events asked for them, which Cloudflare
ends at 15 minutes: a sleep's hold, save and stop fit one alarm, and its
backups get what the rest leaves (`BACKUPS_MS`, 11 minutes 10 seconds).
That is the most any save can have, so it is fixed, not one that follows
a save's progress. Saves over 1 GB moved 9 to 156 MB/s on production
(2026-10-27 to 11-03), so a full disk's save (about 10.8 GB) fits all but
the slowest 6% of tries, each tried again. The snapshot after it is given
what the hold and the save left of the sleep's 14 minutes, at least 30
seconds and at most five (`snapshot_ms`): a snapshot is incremental, but
the first after a disk grew took about 85 s for 9 GB (preview,
2026-11-18), and 30 s for every one lost each over about 1.5 GB. A
snapshot cut short frees nothing sooner: the runtime's takes no signal and
holds the container until it ends. A wake that comes during the stop
starts once the stop ends (`owes_start`), made by its alarm, never by its
owner's sleep, which answers within its steps. The stop's deadline is six
minutes, but every minute (`STOP_LOOK_MS`) its alarm looks whether its
container is gone, so one whose end its isolate lost is found as soon as
before. A start takes an alarm of its
own: the one start an alarm makes (a retry's, or one owed after a sleep),
its disk's read among its steps, with the alarm's other events, fits
15 minutes (`RESTORE_MS` leaves them their 65 seconds). So an owner's
sleep answers within 14 minutes.

### The fragment API

- `http://api.fragment.internal/<path>` is the platform's API
  (docs/api.md), and `…/f/<fragment>/<path>` a fragment's own routes
  (`__live` sockets included), as on its host. A storage token minted
  through it names the code.storage the guest can reach.
- `x-fragment-agent: <agent fragment>` says which agent a request acts
  as. The intercept refuses an agent not assigned to this computer, then
  signs the request (NIP-98) as that agent. It reads a body whole to
  sign its hash, at most 32 MiB, but a blob's upload (`PUT
  /api/f/{name}/blobs/{sha256}`): that streams through with its declared
  length and no payload hash, as from the internet (the signature binds
  the URL, which names the hash), at most `BLOB_MAX_BYTES`.
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
  - On a disk (below, "A disk"): `GET /api/computer` with
    `diskMountMs`, `diskMisses` and `diskRefetches` (each a number, any
    left out), what its mount did, which the start's line takes from its
    first read.
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

A SimpleX door's container has the same relay (`GET /relay?host=&port=`
at `door.fragment.internal`: entry.mjs `DoorEgress`, `DoorRelay.target` in
cell/src/doors.rs, relay.mjs pipes both): the same targets and each
relay's bounds, its runner opening at most `relay::OPEN_MAX` at once. It
keeps no day's budget of its own: a door runs no one's code but the
platform's, and carries only its owner's files and its agent's replies'
files, each bounded by its runner and the door's holding.

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

- `POST /v1/chat/completions`: OpenAI's shape, `model` a route of the
  deployment's by its name (`cheap`, `medium` on Finite's), `vision`,
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
- `x-fragment-tier: <route>` (optional; a route's name, any other 400) is
  the caller's route, by which a model's id, `fallback` and `vision` run
  as its route's.
- **A model by its id.** The view's `models` names each slot's model id
  (`ModelIds`), as the deployment names them now. A guest names its
  models by those ids and reads them again while awake: a change of the
  deployment's `models` is a deploy, no new image, and an id no slot runs
  is refused (400). The route's names keep working.
- **Each route.** The view's `models.routes` names the routes `cheap` and
  `medium` (each the route of that name, else the deployment's default:
  what an image older than the list reads): its label, whether it is
  private, and its fallback's and vision model's ids.
- **A slot's own fallback.** A call that fails before answering is made
  once more on its slot's fallback, however it named the slot, and never
  a third time. A call that streams nothing is the runtime's own to give
  up on.
- **A slot's `private`.** A route on a private slot is a private route:
  every slot its agents' calls reach (its fallback, its vision, its Deep,
  and the view's `fallback`, the default route's, which an image that
  reads no route names for its stall) is private, or the deploy is
  refused. Not private yet: voice
  notes (`whisper`), decisions, the runtime's speech, the vendors its
  tools call (web search, `x_search`, image generation), and an owner's
  own model.
- **Fast and Deep** (decision 65). Each route runs Fast, its own slot, for
  its agents' turns, and Deep for their delegates. `models.routes` carries
  each route's `reasoning` (Fast's effort) and `deep` (`{id, reasoning?,
  fallback?}`), absent when the deployment names none; an image copies
  them into each profile, and one that reads neither runs delegates on
  Fast. A delegate names its Deep by its id with its route said, so a
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
  deploy/example.jsonc has the platform's. A connection's row is a file
  of the checkout (deploy/providers), which a deployment names beside
  its own client id and secret and never copies, so the row ships with
  the release that reads it (deploy/providers/README.md). Connecting one is
  docs/api.md's, Connections; its consent is browser-bound
  (docs/pipes-browser-binding.md), and a connection holds only the scopes
  its person allowed: read only or read and write, as they picked, and
  of those what they left ticked.
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
- **An own key is its person's Vault's** (M16; docs/secrets.md, "A
  person's Vault"), its own kind, used by each agent not narrowed from
  it. The computer asks the Vault for it at each request it swaps, beside
  its ask of the agent's fragment (so it waits on no hop more), and for
  which keys the person holds at each read of a view. A computer from
  before the move held them (`own_keys`, `own_signins`): it hands its keys
  to the Vault at its owner's next read of it or of their connections, or
  its agent's next call through one (never a start, an exit, a takeover or
  its guest's request), and the Vault takes one only where its person said
  nothing since. Until then its views list the keys it still owes. A
  sign-in begun there finishes from there.
- **The intercept** catches the catalog's hosts on HTTPS and HTTP, sends
  on over HTTPS, and swaps each placeholder where its row says it goes
  (`swap::Plan`). It refuses the request, reaching no provider, for a
  placeholder sent to a host not its provider's, in a place its row does
  not name, forged or naming an agent not on this computer now, of a
  provider its agent was narrowed from, or of a connection not connected
  or an own key not given (403 `not_connected`); and an operator key's
  when its owner's gate, asked beside the call as for a model call, has
  heard them refused (402, 403; docs/ledger.md, "The gate"); and one
  past its row's `calls_an_hour` from this computer (429 `rate_limited`,
  with `retry_after_ms`: Google Places, 120 by default, uncharged; a call
  is counted as it is sent, beside it, so one refused first costs none). A body goes
  as it came, and a
  redirect is never followed. A connection's token is asked of Pipes for
  each request, never held.
- **Calls an operator key never goes on.** Every agent shares an operator
  key, so a call that reaches the vendor's whole account (everyone's
  history or live browsers, a key minted from it, its own disable) would
  reach other people's data. Each key is locked at its vendor first; its
  row's `deny` names what the vendor can't scope away (`catalog::Deny`:
  `METHOD /path`, a `*` segment any one, a last `**` the rest). The
  intercept refuses such a call (403 `forbidden`, naming it), reaching no
  provider and charging nothing. It reads the path case aside, decoded,
  doubled and trailing `/` dropped, a `.json`-like suffix as the path,
  and refuses one it can't read plainly (an encoded `/`, a `;`, a `.`
  segment). A GET covers HEAD, and a method override header
  (`X-HTTP-Method-Override`) counts as the method too. It is not an
  allowlist: every other call goes as before, an own key's and a
  connection's are never denied, and an empty list denies nothing. It
  can't catch an account-wide call a vendor adds later; the vendor's own
  scoping is the main lock.
- **An operator key never comes back.** It is the platform's own at its
  vendor, so an answer to a request that carried one names each echo of
  it `{{provider}}`, in its headers and its body, as a Vault's fills are
  named (`Plan::operator_keys`, `pipe::answer`; docs/secrets.md, "Echoes
  are hidden"): nothing the pipe puts on a request comes back in its answer.
- **An account's own host.** A connection whose accounts each live at a
  host of their own (a Salesforce org at its My Domain) names the suffix
  they are under and the Pipes metadata field naming each account's
  (`account_host`). The intercept also catches the suffix's glob
  (`*.my.salesforce.com`); a request to a host under it is swapped only
  when Pipes' credential for the account names that very host
  (`AccountHost::of_metadata`), since anyone may own a host under the
  suffix. Another account's host is refused (403), naming the account's
  own; an account whose metadata names none is refused too.
- **Who may use what.** An agent may use every provider its owner has
  (decision 44); its owner may narrow it to a list, a specialization
  rather than a wall. Where a connection takes several accounts, each
  agent uses the one its owner picked, else the oldest connected; its
  placeholder is the same. Both are the agent fragment's (`meta`
  `grants`, `fragment_core::grants`), changed only by its owner in
  person through their computer's routes. The computer asks the agent's
  fragment for them at each request it swaps and each read of a view,
  and keeps none: a provider taken away is refused from the next request.
  A computer from before held them itself; the rows its agents'
  fragments had not taken when its step 6 dropped them it owes
  (`grants::Owed`): an agent's next ask carries its row, taken whole by a
  fragment holding none of its own and ignored by one that does
  (`grants::answered`), then forgotten.
- Each call a provider answered is counted as its agent's in the owner's
  ledger (docs/ledger.md, "Usage"), after the answer; an operator key's
  is charged to its agent's owner too. A computer from before kept its
  own `uses`; its step 7 keeps the months it held as owed (`meta`
  `uses_owed:<month>`), each handed to the owner's ledger as the owner
  reads the computer's view (never from a start, an exit or a takeover),
  taken once, then forgotten.
- **A person's Vault** (FIN-466; docs/secrets.md, "A person's Vault"):
  each secret bound to an agent, with a host, is one of its credentials,
  kind `own`, named by the secret, with no variable. Its hosts join the
  catalog's in the egress's intercepts: a start catches them before its
  guest runs (asked beside the container's start when its guest is held,
  before it when not; above), and its guest's read of its view catches
  any not caught yet (one set since), those alone, before it answers. A Vault that does not answer lends none
  until a read of the view finds it answering.
  The intercept asks the Vault for it per
  request, plans it beside the catalog's rows, and names each value it
  filled in the answer (`{{name}}`); a Nostr key's is a signature the
  Vault makes. A Vault's calls are not counted in usage.

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
- A WebSocket on a port holds the computer awake only while its page
  speaks: its opening, and each message from the page's end (heard a
  minute at a time), hold it `IDLE_MS`; one whose page says nothing lets
  it sleep, and a page that stops speaking when no one is at it lets it
  sleep under an open tab. The container may speak first; its words hold
  nothing. A close at either end closes the other. A sleep (a restart's
  too) closes each as it begins, before its hold, with 4000 and `asleep`
  or `restarting` (one whose upgrade was still on its way too), so nothing
  a page sends after reaches the container its save holds; a request to a
  port while it sleeps wakes it after, and is answered 503.
- **The screen** (decision 11) is a page on port 6080, one for each
  agent, at `/p/6080/?agent=<agent fragment>`. The platform carries the
  query and reads nothing in it: which desktop is the agent's, and
  refusing an agent the computer does not run, are the image's. The page
  reaches its sockets by relative URLs, naming the agent again. The shell
  tells it whose screen it is by name, as it loads and on every rename:
  `postMessage({fragment: "screen", agent, name})`; a page takes it only
  from its parent and only for its own agent. Opened in a tab of its own,
  it names none: the computer knows an agent by its label, never its name.
  The shell also tells the screens on view that its person is at it
  (`{fragment: "screen", agent, active: true}`, every 30 s at most).
  Our screen backs off to silence (`images/hermes/screen/attend.js`): no
  key, press, pointer or touch on it or the shell for five minutes
  pauses its stream, its last frame kept; the next one resumes it. Once
  its sockets close (a sleep, a restart, the network), only a press or a
  key opens them again, once each, and a computer asleep wakes for them.

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
  it through the files route, never a storage token: the agent edits its
  own fragment, so a viewer's read-only sync is not this one's. A fragment
  shared with the agent's owner it syncs with the `fragment` CLI, as its
  owner would, at the lower of their roles (cli/GUIDE.md, Sync in depth). An agent whose `agent.json` names another runtime (Claude) has no
  profile, desktop or route: the boot reads `runtime` as the bridge
  does (`with_runtimes`), and the ready file names it, for the bridge to
  run on its runtime, once its own files are restored for that runtime
  (below, "Claude Code"). A start reads every agent's before it
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
  `computer-check` puts the copies back and checks each. It also names
  the caches a save leaves out (`held::caches`: package managers'
  downloads under Hermes' home and each agent's, a browser profile's
  caches, never a person's own folder) and Claude Code's versions but the
  one it runs, as many as fit beside its copies.
- **What changes under the hold.** Hermes' gateway still writes on its
  own timers while held, each file replaced whole or appended, so a save
  holds a whole version of it; nothing else under `/data` changes (the
  Docker lane's `held_nothing_under_data_changes`). Its model-catalog
  refresh is off, and Node's compile cache is outside `/data`. A Claude
  agent's idle `claude` is stopped before the hold is answered
  (docs/bridge.md, "The Claude runtime").
- **A turn a restart cut** is settled in its Hermes session before the
  gateway starts, as Hermes' own `/discard` settles it, so the next
  message is a turn of its own and the chat is never paused behind it
  (P5; `hermes::SETTLE_CUT_TURNS`).
- **Its agents change while it runs**, nothing restarted: one assigned
  is served within seconds; one unassigned stops at once, its profile
  retired at the next start.
- **Each agent has its own desktop** (Hermes' Bot Desktop), started for
  its screen's first viewer or its first `computer_use` or browser call,
  and stopped after 10 minutes unused. Take over is that desktop's lease,
  which Hermes' tools obey until Give back. Its agents run as one user
  (decision 44), so only the lease and the platform skill keep an agent
  off another's desktop. The desktop's Browser icon and the agent's
  `browser_exec` share one regular Chromium and one saved profile per
  agent, which shows pages `navigator.webdriver` false, as a person's own
  browser does. It stays up as long as its desktop, whose idle stop closes
  it first, as a person would, its cookies written. A link opened on the
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
until it copies it into its work (`hermes::FILES`); that a voice memo
arrives already transcribed, its words the quote above Hermes' audio
note, its file only for work on the recording (`hermes::VOICE`); and that a
reply line `MEDIA:<absolute path>` attaches a file there
(`hermes::SENDING`). Hermes keeps the system prompt a chat's session
began with and replays it on every later turn (rebuilding it only at a
compression, a model change or `/new`), so whenever what it is built
from changes (the relay hint, or a skill file Hermes indexes into it: the
platform skill's, a bundled one, the owner's managed set;
`hermes::prompt_inputs`), every session is asked to rebuild its prompt
once, on its next turn, as Hermes' own `hermes sessions refresh-prompts`
does (`hermes::REFRESH_PROMPTS`): at boot, its skills settled, against
what the computer last stamped (`hermes::PROMPT_STAMP`), and after each
install of the managed skills while it runs. An open chat has the change
at its next turn, its conversation kept, at one prompt-cache miss; the
same inputs ask nothing.

### Its name, in our Hermes image

An agent's name is its agent fragment's title, and its role its face's
role (docs/api.md, "An agent's name and picture"). The image tells
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
- **Hermes' optional skills** (the image's `/opt/hermes/optional-skills`,
  141 at v0.23.2) are off until the owner switches one on from the shell's
  Skills page: an `optional/hermes/<id>.on` file in their skills fragment.
  Each install reads the switches with the managed set, and the boot
  copies each skill read-only beside the bundled ones; switched off, it
  goes at the next install. One the image lacks, or whose name a managed
  or bundled skill takes, is left out and logged (`skills.optional`).
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
installed under `/data` at run time but what its agents install into its
Python environment (next) and the person's own Claude Code (below). On a
disk ("A disk"), what a language's own tool installs is kept too: a login
shell's npm global prefix, pipx's and uv's tools, and cargo's and Go's
homes are under `/data/work` (`/etc/profile.d/fragment-disk.sh`), their
programs first on its PATH. apt's installs are still the image's. The
bridge runs as root and keeps SimpleX's state from the agent's user,
though not from its sudo.

### Python in our Hermes image

Paul, 2026-11-02 (his agent found no `pip`, `uv` only in Hermes' tool
store, and the image's Python read-only): `pip install` and `uv pip
install` work, and what they install is kept.

- **One environment for the computer**, `/data/work/python`, its agents'
  (they run as one user, decision 44), saved with their work. Every login
  shell, as each command of Hermes' terminal runs in, has it active
  (`/etc/profile.d/fragment-python.sh`: its `bin` first on `PATH`, and
  `VIRTUAL_ENV`), so `python`, `pip` and `uv pip` are its. `uv` and `uvx`
  are Hermes' own, on every `PATH`.
- **Made by the boot**, off its path and offline: uv makes it on the
  image's Python (`/usr/local/bin/python3`, the name upstream links it
  at), and its pip is the wheel that Python bundles
  (`boot/src/python.rs`).
- **Kept** across a restart and a sleep, and across an image update whose
  Python is the same minor release: it runs whichever Python the image
  links (`bin/python` is a link to `/usr/local/bin/python3`, as is its
  `home`), wherever the image's store keeps it. One made for another
  Python (or unreadable) is set aside whole (`/data/work/python.3.14`),
  with `requirements.txt` listing what it held for `pip install -r`, and a
  new one is made; the agent's page says so.
- **The image's own packages**, in `/opt/fragment/python` (the document
  skills' libraries and Playwright; never Hermes' store, whose Python its
  PM records a digest of), are named in it by a `.pth` the boot writes at
  each start, after its own, so an agent's install of one wins. Hermes'
  venv names them by the same `.pth`.

### Playwright and small tools in our Hermes image

Paul, 2026-11-02: an agent's own code drives a browser with Playwright,
on the system Chromium, which passes Cloudflare's bot check where Chrome
for Testing fails.

- **One release for Python and Node**, pinned in
  `images/hermes/playwright-requirements.txt`: Python's in the image's own
  packages (above), its driver on the image's Node
  (`PLAYWRIGHT_NODEJS_PATH`; the wheel's own is dropped), Node's global
  (`require('playwright')` through the login shell's `NODE_PATH`), and the
  `playwright` CLI. A plain
  `chromium.launch()` runs `/opt/fragment/bin/chromium`, the desktop's:
  the image puts it in `PLAYWRIGHT_BROWSERS_PATH` under the names that
  release looks for, marked installed, beside the image's FFmpeg under
  its name (a page's video), so `sudo playwright install chromium`
  downloads nothing. sudo keeps `PLAYWRIGHT_BROWSERS_PATH`, so a browser
  `sudo playwright install` adds is where an agent's own launch looks,
  and `PLAYWRIGHT_SKIP_BROWSER_GC`: that directory is Hermes' tool store,
  and Playwright's install would collect Hermes' own FFmpeg from it (`uninstall`, which always collects, is refused
  there). As
  the agent's user, `playwright install` (either runtime's) answers at
  once (`playwright-node`, the Node both run Playwright's CLI on):
  Chromium, or no browser named, is installed already; another browser is
  root's to install. Playwright's own would wait ten minutes on its lock
  in that directory, root's.
  Another release an agent installs looks for its own names, and finds
  none.
- **`unzip`, `zip`, `jq` and `sqlite3`**, Debian's. `sqlite3` runs with
  Debian's SQLite library, not the newer one upstream puts first.
- Hermes' own Chrome for Testing is not in the image: every browser
  here runs the system Chromium, which passes Cloudflare's bot check where
  Chrome for Testing fails.

### Claude Code in our Hermes image

Paul, FIN-233 (2026-10-31): an agent whose `agent.json` names `claude`
runs on Claude Code, which the person installs on their computer
themselves, as on a VPS of theirs. The image ships none, and no flag
gates it.

- **Installed and signed in from the chat**: a Claude agent's turn on a
  computer with none asks its person to install it, and Install runs
  Anthropic's own installer as below; signed out, its turn's card links
  Anthropic's sign-in for their own browser and takes the code it shows,
  on the computer alone (docs/bridge.md, "The Claude runtime").
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
  directory.
- **Its updates are Anthropic's**: Claude Code updates itself in the
  background, keeping the two newest versions, so a save carries one or
  two (about 270 MB each).
- **A Claude agent has no Hermes profile or desktop** (above, "One
  profile per agent of Hermes'"): its person installs and signs in from
  its chat.
- **Its self is its fragment's, as a Hermes agent's is** (M17, FIN-542):
  the same sync, every 60 s and at each start before the bridge runs it,
  keeps its `SOUL.md` and its own `CLAUDE.md` in its work directory (its
  `AGENTS.md` imports the soul), and Claude Code's auto memory for it as
  the fragment's `memories/claude/`. Each folder keeps what was agreed
  beside its files, so one lost alone deletes nothing, and a file deleted
  from its work directory comes back rather than leaving the fragment. A
  computer whose `/data` is lost or new, or an agent newly assigned to
  it or switched to Claude, has all of it back before the bridge runs the
  agent: the ready file names each agent with the runtime it was made
  ready for, a Claude agent's only once a sync restored its files (a
  failed one is tried again at each read of the computer's agents, every
  3 s), a Hermes agent's whose profile was written while the gateway runs
  only once the running gateway says it serves it (asked with
  `rescan-profiles`: a turn relayed before is refused, and lost), and the
  bridge holds the turns of an agent not ready for the runtime it names,
  never dropping them.

## Billing

Awake time, operator keys' calls and model calls are metered to the
computer's owner, and its saves are not (docs/ledger.md, "Who pays").
Awake time is time the computer was shown up: from its start's `Ready`
(or, for a start put to sleep before it, that sleep), in the intervals
the DO hears, each alarm's at least every five minutes and each step of
a sleep's. An event that comes when nothing was heard of a running
computer for `ALARM_LATE_MS` (twenty minutes: its alarms went unheard)
meters it only to the last moment it was shown up, never the silence
(FIN-511).
At zero credit no wake starts, and a computer already awake runs on
until it sleeps (decision 27).
