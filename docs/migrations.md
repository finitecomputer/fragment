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
| The mover (finite-mono's `finite-mover`) | Nothing here | Pushes the roster, pulls its mailbox, does what a command says, reports. Never acts unasked. |
| The new agent | Its own fragment | Opens its stage, reads the zip by ranges, imports with the `import-agent-home` skill, says it is done. |
| Operators | Nothing | List moves; retry, restart the old agent, retire, withdraw. |

## Credentials

- **The mover** signs with NIP-98, with the key whose npub the
  deployment's `migrations.mover` names. The router takes it only on
  `/api/migrations/mover/*`. It is the operators' check
  (`FRAGMENT_OPERATORS`), but the mover is no operator. Fragment holds no
  secret for it: rotating it is a new npub in the config and a deploy.
- **The new agent** signs with its own key, through its computer's
  egress. Its stage's routes answer only the agent the move is bound to,
  and only while the move is `importing`.
- **Operators** are the deployment's (`orgs.rs` `operator`).

## The mover's routes

| Route | Body → answer | Rules |
|---|---|---|
| `PUT /api/migrations/mover/roster` | `Roster` → `RosterTaken` | A snapshot replaces the last. The same `snapshot` id again changes nothing (`replay`). A row in `listed`, `open`, `later` or `never` that is missing from it is `withdrawn`; a row in flight is left as it is. A person already known here (their WorkOS id among the registry's subjects) is bound at once. Pushed every 10 minutes. |
| `GET /api/migrations/mover/mailbox` | → `Mailbox` | Each command stays until a report on it is taken, so reading again is the retry. Polled every 10 seconds. |
| `POST /api/migrations/mover/agents/{old}/report` | `Report` → `Reported` | Taken only for the old agent's current command. Any other command id is 409 `conflict`, changing nothing. `seq` grows per command: a `seq` taken already is a 200 with `taken: false`. |

A command's id is `<old>:<attempt>:<kind>` (`command_id`). Each old
agent has at most one command at a time: a new one replaces the last.
The mover keys its work by the id, and each step is idempotent on its
side: a stop of a stopped runtime does nothing, and a stage writes the
same keys again.

| Command (`do.kind`) | The mover | Reports |
|---|---|---|
| `stop_and_stage {phase, prefix, secrets}` | Stops the old agent, then writes `<prefix>stage.zip` and, last, `<prefix>manifest.json` to the `STAGE` bucket. `secrets` is empty until the vault (FIN-466). | `stopped`, `progress`, then `staged {manifest_sha256, bytes, files}`, or `failed {step, why}` |
| `retire {as: migrated \| archived}` | Retires it (a verified backup first) and marks it moved | `retired {receipt}` |
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
- The new agent reads the zip only through the platform. The mover
  writes it with a token scoped to the bucket. Nothing else reads it:
  no public bucket, no presigned URL.

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
| listed → open | the bind: a roster push that finds the person (and, with F5, their sign-in) | none | none |
| open → later, never, asked | the person (F4, F6) | none | none |
| later → open | the alarm | 24 hours | offered again |
| asked → staging | the mover's `stopped` | 1 hour | held, `mover_down` |
| staging → importing | the mover's `staged` | 30 minutes without progress; 12 hours in all | attempt + 1, up to 3, then held `stage_failed` |
| importing (not opened) | the agent's `open` | 2 hours after the routine | the routine once more, then held `not_opened` |
| importing → retiring | the agent's `done ok` | 24 hours | held, `import_timed_out` |
| retiring → moved | the mover's `retired` | 24 hours | held, `retire_timed_out` |
| a staged attempt's objects | the alarm | 14 days after `staged` | deleted (the old host's backup remains) |
| archived → expired | the alarm | cutoff + 90 days | deleted |

A `failed` report counts as an attempt, as a stall does. Staff act on a
held move: `retry` (a new attempt), `restart-old` (the old agent runs
again, the offer is open again), `retire` (the import went well enough
by hand), and `withdraw` (out of the move).

## Deletion

| When | What |
|---|---|
| `done`, either way | that attempt's prefix: its routes refuse at once, its objects go from the alarm, at most 1,000 a pass |
| 14 days after `staged` (phase 1, done or not) | the same |
| a superseded attempt | its prefix |
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
| `POST /api/admin/migrations/{old}/retry` | `AdminMove`: a held move asked again, as a new attempt |
| `POST /api/admin/migrations/{old}/restart-old` | the old agent started again, the move offered again |
| `POST /api/admin/migrations/{old}/retire` | the old agent retired as migrated |
| `POST /api/admin/migrations/{old}/withdraw` | the move out of the roster's hands: offered no more |

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

The cell reads it as `FRAGMENT_MIGRATIONS`. The cutoff (phase 2) and
the credit grant come with their own PRs.
