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
| Operators | Nothing | List moves; retry, restart the old agent, retire, withdraw. |

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

## The mover's routes

| Route | Body → answer | Rules |
|---|---|---|
| `PUT /api/migrations/mover/roster` | `Roster` → `RosterTaken` | A snapshot replaces the last. The same `snapshot` id again changes nothing (`replay`). A row in `listed`, `open`, `later` or `never` that is missing from it is `withdrawn`; a row in flight is left as it is. A person is bound when they open the move (the WorkOS user they signed in as, asked of the registry then); a new row of a person bound already is offered at once (`bound`). Pushed every 10 minutes. |
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

A command's id is `<old>:<attempt>:<kind>` (`command_id`). Each old
agent has at most one command at a time: a new one replaces the last.
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
| `POST /api/migrations/stages/{stage}/done` | `StageDone {ok, report_sha256?, why?}` → `MoveAt` | `ok` moves it to `retiring`; not ok holds it for staff. The same `ok` again changes nothing. Either way the stage's routes refuse at once, and its objects are deleted. |

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
withdrawn: missing from a roster while listed, open, later or never
```

The pure machine is `fragment_core::migrations`; its deadlines are its
constants. Migrations' alarm is the one supervisor: it wakes at the
earliest deadline of any row.

| From → to | Moved by | Deadline | When it passes |
|---|---|---|---|
| listed → open | the bind: the person opens the move, signed in as the WorkOS user the roster names (or a roster names a new row of a person bound already) | none | none |
| open → later, never, asked | the person (F4, F6) | none | none |
| later → open | the alarm | 24 hours | offered again |
| asked → staging | the mover's `stopped` | 1 hour | held, `mover_down` |
| staging → importing | the mover's `staged` | 30 minutes without progress; 12 hours in all | attempt + 1, up to 3, then held `stage_failed` |
| importing (not opened) | the agent's `open` | 2 hours after the routine | the routine once more, then held `not_opened` |
| importing → retiring | the agent's `done ok` | 24 hours | held, `import_timed_out` |
| retiring → moved | the mover's `retired` | 24 hours | held, `retire_timed_out` |
| a staged attempt's objects | the alarm | 14 days after `staged` | deleted (the old host's backup remains) |
| archived → expired | the alarm | cutoff + 90 days | deleted |

A stage's `failed` report counts as an attempt, as a stall does. A
retirement's `failed` keeps its command for the mover to try again: the
moved mark it ends in is what finite.computer redirects by, so only its
day's deadline holds it for staff. Staff act on a
held move: `retry` (a new attempt), `restart-old` (the old agent runs
again, the offer is open again), `retire` (the import went well enough
by hand), and `withdraw` (out of the move).

## Deletion

| When | What |
|---|---|
| `done`, either way | that attempt's prefix: its routes refuse at once, its objects go from the alarm, at most 1,000 a pass |
| 14 days after `staged` (phase 1, done or not) | the same |
| a superseded attempt | its prefix, and its unfinished upload aborted |
| Never | nothing: Never only stops the asking |
| 90 days after the cutoff (phase 2) | the same |
| Backstop | the bucket's lifecycle rules: `p1/` after 16 days, `p2/` after 100, and incomplete multipart uploads after 1 |

`/data` saves never hold a stage: the importer reads it by ranges into
`/tmp`, outside the saved directories (docs/computers.md).

## The operators' routes

`/api/admin/migrations`, from the CLI: the whole admin surface. There
is no page, no Glance column, and nothing is pushed anywhere else:
staff read a move's state and act on it here.

| Route | Answer |
|---|---|
| `GET /api/admin/migrations[?after=<old>]` | `AdminMoves`: each move by old id, a page at a time |
| `POST /api/admin/migrations/{old}` `AdminAct {op}` | `AdminMove`, after `op`: `retry` (a held move asked again, as a new attempt), `restart-old` (the old agent started again, the move offered again), `retire` (the old agent retired as migrated), or `withdraw` (the move out of the roster's hands: offered no more) |

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
`FRAGMENT_MIGRATIONS_CUTOFF_MS`. The credit grant comes with its own PR.
