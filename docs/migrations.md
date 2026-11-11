# The finite.computer move

How finite.computer's agents come to this platform (FIN-463). One
`Migrations` Durable Object owns every old agent's move; everyone else is
asked. All of it is removable once the move ends: the debt ledger's
entry, "The finite.computer move", says when and in what order.

The shapes are `fragment_proto::migrations` (crates/proto/src/migrations.rs).
The routes the mover and a new agent call are on the wire (docs/wire,
tag `migrations`), so a change an older client would feel fails
`cargo xtask check`. The operators' routes are not on the wire, as no
admin route is.

## Who does what

| Actor | Owns | Does |
|---|---|---|
| `Migrations` (one object, `migrations`) | The roster's rows, each old agent's state, its attempts and deadlines, the mover's mailbox | Decides every transition. Its alarm is the only clock. |
| The mover (finite-mono's `finite-mover`) | Nothing here | Pushes the roster, pulls its mailbox, does what a command says (a stage it uploads through the platform), reports. Never acts unasked. Holds no bucket credential, and reads no stage. |
| The new agent | Its own fragment | Opens its stage, reads the zip by ranges, imports with the `import-agent-home` skill, says it is done. |
| Operators | Nothing | List moves; retry, restart the old agent, retire, withdraw (the will-not-move list), include. |

## Credentials

- **The mover** signs with NIP-98, with the key whose npub the
  deployment's `migrations.mover` names. The router takes it only on
  `/api/migrations/mover/*`. It is the operators' check
  (`FRAGMENT_OPERATORS`), but the mover is no operator. Fragment holds no
  secret for it: rotating it is a new npub in the config and a deploy.
- **The old platform holds no bucket credential.** R2 has no write-only
  object permission, so a token on the old hosts could read every stage.
  The mover uploads through the platform instead, which writes the
  `STAGE` bucket through its binding alone.
- **The new agent** signs with its own key, through its computer's
  egress. Its stage's routes answer only the agent the move is bound to,
  and only while the move is `importing`.
- **Operators** are the deployment's (`orgs.rs` `operator`).

## The offer (F4)

A person whose old agents' moves are `open` is asked about each, oldest
first, in `GET /api/notices` (docs/api.md, Notices): a notice that must
be answered, `move:<old>`, its words the server's
(`fragment_core::migrations::offer`): "Bring <name> over from
finite.computer?", "You'll get a month free.", with the roster's picture
through the platform. Its answers:

| Answer | Does |
|---|---|
| Yes | Opens `/move` (F6), which asks what the import needs, makes the new agent, and posts `yes {agent}` |
| Not now | `POST /api/migrations/offers/{old}/later`: asked again after `LATER_MS` (a day) |
| Never | `POST …/never`: asked no more until `POST …/ask-again` (from `/move`) |

The routes are the person's own (a session or their key): another
person's old agent is none (404), an agent is refused (403), and an
answer its move's state does not take is 409 in the machine's words.
`GET …/picture` reads the roster's `picture_url` for its person alone
(https; an image a page shows, at most 1 MiB).

**Binding.** A person is bound by their first notices read: the router
asks Migrations for their offers, and a person it does not know yet is
looked up once (the WorkOS user they sign in as, of the registry) and
bound by it, roster or not. From then on a roster row of theirs is
offered as it comes. A person with no WorkOS user, or whose user is
another person's here, is told nothing.

**Told.** Every change to a bound person's move (their answers, a roster,
the alarm's deadlines, staff's acts) tells their open pages
(`principal::tell_changed`, at most 32 people a call), which read their
notices again.

## The page (F6)

`/move`, a view of the shell's (cell/shell/move-page.js), reached from
the offer's Yes, and from Settings while the person has old agents. On
it, the offer's cover stands aside: it is answered there.

- `GET /api/migrations/offers` (`Offers`): the person's old agents, each
  where its move is in the server's words (`fragment_core::migrations::said`):
  on finite.computer, stopping, copying, bringing over, done, paused for
  staff. The page reads it again every few seconds while a move is under
  way.
- The one question: which to bring over. Each new agent takes its old
  one's name (the import brings who it is; it is renamed as any agent). A
  Never'd one has Ask again.
- Bring over: `POST /api/migrations/offers` opens the move (binds them;
  the comp, once), the page waits for the seat to reach the ledger, then
  for each, one at a time, a new agent is made as Settings' Add makes one
  (`addAgent`, its chat made but not opened), and `…/offers/{old}/yes
  {agent}` posted: the old agent is stopped then. One new agent per old
  agent: its agent.json keeps the old one it was made for (`from`), and
  the page hands that agent back to Add (`agent`), which goes on with
  what it lacks, its chat too; one cut short before its agent.json is
  found by its name, the old one's each time. A Yes that never arrived, a
  reload, or a lost answer never makes a second. An agent.json that could
  not be read (not missing) makes nothing: the page stops, to be tried
  again. A refusal shows the server's words.

## The comp (F5)

A person on the roster who opens the move (`/move`, `migrations::visit`)
and has an old agent the roster offers or moves is given, once:

- `COMP_GRANT_MICROS` ($25) of credit on their ledger, `GrantCredit` by
  `migrations` with id `move:<person>` (`comp_grant_id`): a ledger answers
  the same id again as it did, so it is granted once a person, ever;
- a comped `seat` for `COMP_MS` (30 days, `CompSeat.untilMs`), which the
  registry ends then. One who holds a seat already keeps theirs.

Then Migrations marks them (`people.comped_ms`) and no visit gives it
again. A visit cut off before the mark gives it again, each step
answering as it did. A guest makes no agent (decision 25), so the comp
comes before `/move` makes the new ones.

## The mover's routes

| Route | Body → answer | Rules |
|---|---|---|
| `PUT /api/migrations/mover/roster` | `Roster` → `RosterTaken` | A snapshot replaces the last. The same `snapshot` id again changes nothing (`replay`). A row in `listed`, `open`, `later` or `never` that is missing from it is `withdrawn`; a row in flight is left as it is, and one staff withdrew stays theirs (the will-not-move list). A person is bound by their first notices read (the WorkOS user they signed in as, asked of the registry then: "The offer"); a new row of a person bound already is offered at once (`bound`). Pushed every 10 minutes. |
| `GET /api/migrations/mover/mailbox` | → `Mailbox` | Each command stays until a report on it is taken, so reading again is the retry. Polled every 10 seconds. |
| `POST /api/migrations/mover/agents/{old}/report` | `Report` → `Reported` | Taken only for the old agent's current command. Any other command id is 409 `conflict`, changing nothing. `seq` grows per command: a `seq` taken already is a 200 with `taken: false`. |
| `PUT /api/migrations/mover/stages/{stage}/parts/{n}?sha256=<hex>` | the part's bytes → `StagePart {n, bytes, sha256, etag}` | One part of the zip (1 to `PARTS_MAX`, at most `PART_MAX_BYTES`, each but the last at least `PART_MIN_BYTES`), streamed unread straight into the stage's multipart upload in `STAGE`. Its SHA-256, which the signed URL names, is kept with it for the mover's resume; the zip's own SHA-256 in the manifest is what the importer checks. The same part again replaces it. |
| `GET /api/migrations/mover/stages/{stage}/parts` | → `StageParts {parts}` | The parts taken so far: their numbers, sizes, hashes and etags, never their bytes. What a resumed upload sends next. |
| `POST /api/migrations/mover/stages/{stage}/complete` | `StageComplete {manifest}` → `StageCompleted {manifest_sha256, zip_bytes}` | The zip made whole from the parts taken (their bytes in all the manifest's `zip_bytes`; R2 refuses a part too small), then `manifest.json` written, last. Then the mover reports `staged` with that SHA-256. The same complete again answers the same. One cut off between the zip and its manifest is not made again: its attempt stalls, and the next stages afresh. |

The three stage routes take only the stage its old agent's current
`stop_and_stage` names (any other stage is 403), and only while it stages:
`staging` after `stopped`, or phase 2's `archiving` (409 otherwise). No
route gives a stage's bytes to the mover. The platform keeps the
multipart upload's id and each part taken in `Migrations`.

A command's id is `<old>:<attempt>:<step>` (`command_id`): its step is
its kind (`stop_and_stage`, `restart`), or a retirement's with its mark
(`retire_migrated`, `retire_archived`). Phase 2 retires an old agent twice
in one attempt (as archived, then as migrated after a late import), and
no two commands share an id. Each old agent has at most one command at a
time: a new one replaces the last.
The mover keys its work by the id, and each step is idempotent on its
side: a stop of a stopped runtime does nothing, and a stage writes the
same keys again.

A command is flat: its `kind`, and that kind's fields. So is a report:
its `kind` (text, one shape for all), and that kind's fields.

| Command (`kind`) | The mover | Reports (`kind`) |
|---|---|---|
| `stop_and_stage`, `phase`, `stage`, `secrets` | Stops the old agent, then uploads its zip as `stage` in parts (resuming from `GET …/parts`), then completes it with its manifest. `secrets` is empty until the vault (FIN-466). | `stopped`; `progress` with `bytes`, `files`; then `staged` with `manifest_sha256`, `bytes`, `files`; or `failed` with `step`, `why` |
| `retire`, `as` (`migrated` or `archived`) | Retires it (a verified backup first) and marks it moved | `retired` with `receipt` |
| `restart` | Starts it again (staff gave it back) | `restarted` |

The secrets hand-off (`POST …/agents/{old}/secrets`, `Handoff`) waits
for the vault (FIN-466): its types are here, its route is not.

## A new agent's stage

The stage's id is `<old>.<phase>.<attempt>` (`stage_id`). It names the
move in the routine that starts the import.

| Route | Answer | Rules |
|---|---|---|
| `POST /api/migrations/stages/{stage}/open` | `StageOpened {stage, manifest}` | The manifest as the mover wrote it. |
| `GET /api/migrations/stages/{stage}/zip` | the zip's bytes | `Range: bytes=…` is answered 206 with that range alone. |
| `POST /api/migrations/stages/{stage}/done` | `StageDone {ok, report_sha256?, why?}` → `MoveAt` | `ok` moves it to `retiring`; not ok holds it for staff. The same `ok` again changes nothing. Either way the stage's routes refuse at once. A phase 1 stage's objects are deleted; a phase 2 stage's are kept until 90 days after the cutoff (Deletion). |

Each is the bound agent's alone (403 for anyone else), for its current
stage alone (404 for another attempt's), and only while the move is
`importing` (409 otherwise).

### The stage

One zip64 per old agent and attempt, in the deployment's `STAGE`
bucket (`fragment-migrations-stage`, a branch's
`fragment-migrations-stage-<branch>`):

```
p1/<old>/<attempt>/stage.zip       the move class, streamed by the host reader
p1/<old>/<attempt>/manifest.json   written LAST: its presence means staged
p2/…                               the same, for phase 2
```

- Keys name the old agent by Core's opaque id (`valid_old`), never a
  name or an email.
- Each attempt has its own prefix, so a retry never mixes with a
  half-written one.
- The zip holds the collector's `move` class: history and work, owned
  brains and sites. Caches, the runtime and secrets never leave the old
  host (they stay in its encrypted backup). The manifest lists what was
  left out and why (`left`, at most `MANIFEST_LEFT_MAX`).
- `manifest.json` is `Manifest`: `format` (`finite-stage/1`), the old
  agent, phase and attempt, the `layout` (`finite-computer`), the zip's
  size and SHA-256, the files inside, and `left`.
- Only the platform touches the bucket, through its binding: the mover
  uploads through its routes, and the bound agent reads through its own.
  No public bucket (`xtask deploy` refuses one with an r2.dev URL or a
  custom domain), no presigned URL, no token.

### SimpleX in a stage

An old agent's SimpleX conversation with its owner can come with it, so
their SimpleX app keeps talking to the same contact, with no code to scan
(FIN-463). The mover stages it only when the old agent has exactly one
approved owner contact (Hermes' `simplex-approved.json` names one decimal
`contactId`), both databases are plain SQLite (no passphrase), and no
`-journal` or `-wal` file is beside them (its core stopped cleanly).
Then the zip holds exactly these three files, and nothing else under
`.finite-stage/simplex/`:

```
.finite-stage/simplex/chat.db      the old core's identity_chat.db, as it is
.finite-stage/simplex/agent.db     its identity_agent.db, as it is
.finite-stage/simplex/owner.json   {"contact": <the owner's contactId, an integer>}
```

Each stored or deflated, unencrypted; each database at most 256 MiB.
`owner.json` holds that one key. Anything else under the prefix, or one
of the three missing, and the bridge keeps none of it. The old core
(v7.0.2) and the new image's (v7.0.3) share their database version, so
the files open as they are.

- **The agent never imports it.** The import skill's layout lists
  `.finite-stage/**` as never read (`import-agent-home`).
- **The stage's SimpleX files are read before the routine reaches the
  agent.** Its computer's bridge holds the move's routine (its words are
  `fragment_core::migrations::routine_text`) until it has set them aside,
  read by ranges as the agent, or given up (then Link SimpleX, as before),
  so the import, and its `done`, come after by construction. It runs
  nothing then.
- **Only the owner adopts it**, from their Settings ("Keep my SimpleX
  chat"), by a door ticket the platform signs (`adopt`, docs/api.md, "An
  agent's doors"). The bridge binds its link to that contact and lets the
  old agent's other contacts, groups and address go (docs/bridge.md,
  "SimpleX").
- **The old core must not run again** on its copy: the two would take
  turns at the same queues. A `restart-old` after an adoption breaks the
  new agent's conversation until its bridge restarts its core (FIN-483).

### Who may read a stage

Only the one person's bound agent: the agent Migrations bound at their
Yes, for that person's own stage, while it is `importing`. Everyone else
is refused, and the e2e lanes prove each:

- a stranger, another person's agent, and the same person's other agent
  (403);
- the bound agent before the move imports, and after it is done (409);
- an operator: no operator route answers a stage's bytes, only states
  and sizes;
- the mover: no route answers it a stage's bytes, and it uploads only to
  the stage it was told to.

A phase 2 stage (`archived`) is readable only once its person signs up,
says Yes, and so binds their agent. Staff's Cloudflare access to the
bucket is accepted (Paul, 11-09).

## The state machine

```
listed ──bind──► open ◄──── 24 h ──── later
                  ├── Not now ──────► later
                  ├── Never ──► never ──Ask again──► open
                  └── Yes{agent} ──► asked ──stopped──► staging ──staged──► importing ──done ok──► retiring ──retired──► moved
                                       │                  │                   │  done !ok            │
                                       └── deadline, or failed ×3 ────────────┴────────► held ◄─────────┘
                                                                                          │ retry       → asked (attempt+1)
                                                                                          │ restart-old → open (the old agent runs again)
                                                                                          └ retire      → retiring
cutoff (phase 2): listed|open|later|never ──► archiving ──staged, retired──► archived ──Yes{agent}──► importing ──► … ──► moved
                                                                                └── cutoff + 90 days ──► expired (stage deleted)
withdrawn: missing from a roster while listed, open, later or never; or by staff (withdraw), ahead of any roster too
```

The pure machine is `fragment_core::migrations`; its deadlines are its
constants. Migrations' alarm is the one supervisor: it wakes at the
earliest deadline of any row, and at the cutoff while an unasked row
waits for it and fewer than 8 archive.

| From → to | Moved by | Deadline | When it passes |
|---|---|---|---|
| listed → open | the bind: the person's first notices read, or their visit to `/move`, signed in as the WorkOS user the roster names (or a roster names a new row of a person bound already) | none | none |
| open → later, never, asked | the person's routes (F4: `/api/migrations/offers/{old}/…`; Yes from F6's page) | none | none |
| later → open | the alarm | 24 hours | offered again |
| asked → staging | the mover's `stopped` | 1 hour | held, `mover_down` |
| staging → importing | the mover's `staged` | 30 minutes without progress; 12 hours in all | attempt + 1, up to 3, then held `stage_failed` |
| importing (not opened) | the agent's `open` | 2 hours after the routine | the routine once more, then held `not_opened` |
| importing → retiring | the agent's `done ok` | 24 hours | held, `import_timed_out` |
| retiring → moved | the mover's `retired` | 24 hours | held, `retire_timed_out` |
| a staged attempt's objects | the alarm | 14 days after `staged` | deleted (the old host's backup remains) |
| archived → expired | the alarm | cutoff + 90 days | deleted |
| a phase 2 stage, its move ended any other way | the alarm | cutoff + 90 days | deleted |

A stage's `failed` report counts as an attempt, as a stall does. A
retirement's `failed` keeps its command for the mover to try again: the
moved mark it ends in is what finite.computer redirects by, so only its
day's deadline holds it for staff. Staff act on a
held move: `retry` (a new attempt; a held phase 2 import, its kept stage
again, while it is kept), `restart-old` (the old agent runs
again, the offer is open again), `retire` (the import went well enough
by hand), and `withdraw` (out of the move).

## Deletion

| When | What |
|---|---|
| `done`, either way, in phase 1 | that attempt's prefix: its routes refuse at once, its objects go from the alarm, at most 1,000 a pass |
| 14 days after `staged` (phase 1, done or not) | the same |
| a superseded attempt | its prefix, and its unfinished upload aborted |
| Never | nothing: Never only stops the asking |
| 90 days after the cutoff (phase 2), whatever its import did | the same. A phase 2 stage is the only copy left (its old agent was retired as archived), so it is kept until then: after a good import as after a failed one (for staff's retry). Its `done` ends only its reads |
| Backstop | the bucket's lifecycle rules: `p1/` after 16 days, `p2/` after 100, and incomplete multipart uploads after 1 |

`/data` saves never hold a stage: the importer reads it by ranges into
`/tmp`, outside the saved directories (docs/computers.md).

## The operators' routes

`/api/admin/migrations`, from the CLI: the whole admin surface. There
is no page, no Glance column, and nothing is pushed anywhere else:
staff read a move's state and act on it here.

| Route | Answer |
|---|---|
| `GET /api/admin/migrations[?after=<old>]` | `AdminMoves`: each move by old id, a page at a time, with staff's `why` on each they withdrew |
| `POST /api/admin/migrations/{old}` `AdminAct {op, why?}` | `AdminMove`, after `op`: `retry` (a held move asked again, as a new attempt), `restart-old` (the old agent started again, the move offered again), `retire` (the old agent retired as migrated), `withdraw` with `why` (the move out of the roster's hands: offered no more), or `include` (staff's withdrawal taken back); `null` once the move is gone |

### The will-not-move list

The old agents that must not move are the moves staff withdrew: the
one list, in `Migrations`, with nothing kept on the old side.

```
fragment operator moves [--withdrawn]       every move, or the list alone, each with its why
fragment operator withdraw <old> --why "…"  out of the move
fragment operator include <old>             back in
```

- **Withdraw** says why: 1 to `WHY_MAX_BYTES` (200) bytes, trimmed,
  else 400. The same why again changes nothing; another replaces it. It
  takes a move that is listed, open, later, never, held or withdrawn by
  a roster (one in flight is 409: restart the old agent first), and an
  old id no roster has listed yet: its row waits for the roster, which
  fills in whose it is and leaves it withdrawn.
- **A withdrawn move** is never offered, its person's Yes, Not now and
  Never are refused (409), the cutoff passes it by (never archived or
  retired), and the mover gets no command for it. A roster that lists it
  again, or drops it, leaves it withdrawn. A restart pending when staff
  withdrew it stays, so its old agent still starts.
- **Include** takes back staff's withdrawal, and only that: the move is
  offered again, `open` for a person here, else `listed`. One withdrawn
  ahead of any roster is forgotten (its row deleted). One the roster
  dropped stays withdrawn, the roster's, until a roster lists it. One
  whose move began (its person said Yes, or the cutoff took it) is
  refused (409): its old agent may be stopped or retired, and a phase 2
  stage is the only copy left, so it is brought over by hand. On any other
  move it changes nothing.
- **Its limit:** it stops the automated move alone. Someone can still
  move their own agent by hand with the phase 0 guides
  (move.finite.place): that is their own data.

## Configuration

The deployment's config, `migrations` (absent: no Migrations routes
answer but 404, and `deploy` makes no bucket):

```jsonc
"migrations": {
  // the mover's key (an npub): its NIP-98 signature is the mover's.
  // The mover holds the key; fragment holds no secret for it
  "mover": "npub1…"
}
```

`cutoff_ms`, once set, is phase 2's cutoff, in ms since the epoch. The
cell reads them as `FRAGMENT_MIGRATIONS_MOVER` and
`FRAGMENT_MIGRATIONS_CUTOFF_MS`.
