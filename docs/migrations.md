# The finite.computer move

How finite.computer's agents come to this platform (FIN-463). One
`Migrations` Durable Object owns every old agent's move; everyone else is
asked. All of it is removable once the move ends: the debt ledger's
entry, "The finite.computer move", says when and in what order. Its last
section, "What the next migration keeps", outlives it.

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
(`fragment_core::migrations::offer`): "<name>'s upgrade is ready", "Its
memories, chats, and supported files and skills come with it, to a more
secure home with a fresh key.", with the roster's picture through the
platform. A person reads the move as their
agent's upgrade: the same agent, in a better home. The words promise
only what comes (files past the cap, and a skill that needs what this
platform lacks, stay behind), and its key is new: no words promise the
same one. Its answers:

| Answer | Does |
|---|---|
| Upgrade <name> (Yes) | Opens `/move` (F6), which asks what the import needs, makes the new agent (or takes one the person has), and posts `yes {agent}` |
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
(`principal::calls::Changed`, at most 32 people a call), which read their
notices again.

## The page (F6)

`/move`, a view of the shell's (cell/shell/move-page.js), reached from
the offer's Yes, and from Settings ("Agent upgrades") while the person
has old agents. On
it, the offer's cover stands aside: it is answered there.

- `GET /api/migrations/offers` (`Offers`): the person's old agents, each
  where its move is in the server's words (`fragment_core::migrations::said`):
  its upgrade ready, pausing, packing up, upgrading, upgraded, paused for
  staff (with nothing lost only while its stage is kept). The page reads
  it again every few seconds while a move is under way.
- The one question: which to upgrade. Each new agent takes its old
  one's name (the import brings who it is; it is renamed as any agent). A
  Never'd one has Ask again.
- **Into an agent they have.** Someone with an agent here already (one
  who moved by hand, Paul 11-19) is asked, for each old agent: "You
  already have an agent here. Make a new one for this upgrade, or use
  <name>?", its answers "Make a new one" and each of theirs, nothing
  chosen for them; Upgrade waits for each answer. Theirs are their own
  agents on a computer and on Hermes (the import is a Hermes skill) that
  no move is bound to, and none made for another old agent still to
  answer. An agent takes in one move, ever: two would share its import
  folder, its SimpleX staging and its keys' names in the Vault, and
  staff's retry of the first could start while the second imports. The
  server refuses a Yes into an agent another move is bound to (409,
  `fragment_core::migrations::yes_into`), whatever that move's state,
  and staff's retry that would stage or import into such an agent again
  (`retry_into`; one that only retires goes on), and choosing it for a
  second old agent on the page takes it from the first. One of theirs is taken as it is: its Yes alone, nothing made,
  nothing written to it. Until that Yes lands nothing is decided, so a
  reload asks again; once it lands the move names the agent (the
  offer's `agent`), and nothing is asked. An old agent that an agent was
  made for already (`from`, bound to no other move) is not asked: that
  agent is its answer. Upgrade goes on only with old agents the move's
  opening still answers askable (another tab may have answered one).
  The import into one of theirs adds to it and never replaces what it
  was (`import-agent-home`, "You may be someone already").
- Upgrade: `POST /api/migrations/offers` opens the move (binds them;
  the comp, once), the page waits for the seat to reach the ledger, then
  for each, one at a time, a new agent is made as Settings' Add makes one
  (`addAgent`, its chat made but not opened), or one of theirs taken, and
  `…/offers/{old}/yes {agent}` posted: the old agent is stopped then. One
  new agent per old agent: its agent.json keeps the old one it was made
  for (`from`), and the page hands that agent back to Add (`agent`),
  which goes on with what it lacks, its chat too; one cut short before
  its agent.json is found by its name, the old one's each time. A Yes
  that never arrived, a reload, or a lost answer never makes a second.
  Upgrade reads each of their agent.json files first: one that could not
  be read (not missing) makes nothing, and the page stops, to be tried
  again; a question new since the page drew is asked first. A refusal
  shows the server's words.
- Once its new agent exists, a move is that agent's to tell: it imports
  in its own chat, and asks its person there. Each row with a new agent
  (the offer's `agent`) has Open chat, that agent's own chat. An import
  that starts while the page shows takes the person to its agent's chat
  (the first, if several start between two reads); one already importing
  when the page opens leaves the page as it is.
- Its new agent is empty until the import's routine runs, minutes after
  its Yes, so its first message (its hello) says its upgrade is under
  way, and that it will write again once everything is here. Until then
  it answers nothing as if it knew its person: it says it is still
  bringing itself over (`onboarding.js` `helloText`; FIN-492, 11-22: one
  greeted its person as a stranger, in a language not theirs). Every
  agent's hello names the language their browser is set to.
- That hello is said only once its Yes has landed (FIN-817): the page
  posts it after the Yes, and a page opened later posts one lost only
  once the move names the agent (the offer's `agent`). Its making says
  no hello of its own, so a Yes that never arrived promises nothing: the
  chat stays quiet, and Upgrade, pressed again, goes on with the same
  agent, then its Yes, then its hello.

## The comp (F5)

A person on the roster who opens the move (`/move`, `migrations::visit`)
and has an old agent the roster offers or moves is given, once, a comped
`seat` with no end (Paul, 2026-11-19: most of finite.computer's people
are paid for out of band, so everyone moves comped, and staff sort out
their orgs after). It lasts until staff end it or the person takes an
org's seat. One who holds a seat already keeps theirs, as it is. No
credit is granted: a `seat` brings its own each month
(`fragment_core::ledger::SEAT_INCLUDED`). The $25 granted before then
(`GrantCredit` by `migrations`, id `move:<person>`) stays.

Then Migrations marks them (`people.comped_ms`) and no visit gives it
again. A visit cut off before the mark gives it again, the registry
answering the same comp. A guest makes no agent (decision 25), so the comp
comes before `/move` makes the new ones.

**SimpleX needs always-on.** A `seat`'s computer sleeps, and SimpleX runs
only on one that stays on, so a mover whose stage carries their SimpleX
chat would lose it (a mover's, 2026-11-20). The mover says so in the
manifest (`simplex: true`, "SimpleX in a stage"), and the stage's
`complete` makes that person's comped `seat` a `seat_always_on` before its
zip is made: a failure is the mover's complete tried again. A paid seat,
one always-on already, or none is left as it is, and so is a person
wiped since the move bound them: their stage lands as any other.
Everyone else keeps a `seat` that sleeps.

Comps the move gave before with a 30-day end lost it at the registry's
step 12 (`0012_move_comps_open.sql`): each row whose last comp in the
operators' log is Migrations', or, none logged, that Migrations added.
`people.comp_until_ms` is that release's, read by nothing now.

**After the move.** A move's comp is a comp like any other:

- `fragment operator uncomp <seat>` ends it at once (its id, as
  `fragment operator orgs` and `people` print it).
- An org that pays by Stripe offers them a seat (`POST /api/org/seats`).
  Their yes moves them: their org of one ends, the comp with it.
- `fragment operator comp <email> --org <org>` places only someone in no
  org. Someone the move comped is in an org of one already: 409.

## The mover's routes

| Route | Body → answer | Rules |
|---|---|---|
| `PUT /api/migrations/mover/roster` | `Roster` → `RosterTaken` | A snapshot replaces the last. The same `snapshot` id again changes nothing (`replay`). A row in `listed`, `open`, `later` or `never` that is missing from it is `withdrawn`; a row in flight is left as it is, and one staff withdrew stays theirs (the will-not-move list). A person is bound by their first notices read (the WorkOS user they signed in as, asked of the registry then: "The offer"); a new row of a person bound already is offered at once (`bound`). Pushed every 10 minutes. |
| `GET /api/migrations/mover/mailbox` | → `Mailbox` | Each command stays until a report on it is taken, so reading again is the retry. Polled every 10 seconds. |
| `POST /api/migrations/mover/agents/{old}/report` | `Report` → `Reported` | Taken only for the old agent's current command. Any other command id is 409 `conflict`, changing nothing. `seq` grows per command: a `seq` taken already is a 200 with `taken: false`. |
| `POST /api/migrations/mover/agents/{old}/secrets` | `Handoff` → `HandedOff` | The old agent's secrets, once it is stopped, each into its person's Vault within the call: "The secrets hand-off". |
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
| `stop_and_stage`, `phase`, `stage`, `secrets` | Stops the old agent, hands off the `secrets` it names (`POST …/secrets`), then uploads its zip as `stage` in parts (resuming from `GET …/parts`), then completes it with its manifest. | `stopped`; `progress` with `bytes`, `files`; then `staged` with `manifest_sha256`, `bytes`, `files`; or `failed` with `step`, `why` |
| `retire`, `as` (`migrated` or `archived`) | Retires it (a verified backup first) and marks it moved. As migrated, only on staff's word | `retired` with `receipt` |
| `restart` | Starts it again (staff gave it back) | `restarted` |

### The secrets hand-off (F11)

A person's Yes brings their old agent's own keys to their Vault
(FIN-466; docs/secrets.md, "A person's Vault"), so its Telegram bot
keeps talking in the same chat after the move, with no token pasted or
revoked.

- **What.** A phase 1 `stop_and_stage` names in `secrets` every kind:
  `telegram_bot_token`, `openrouter_key`, `nostr_nsec`. A Yes hands off
  each the old agent holds, with no question of its own: the Vault is
  the person's, and each entry is bound to the new agent alone. A key is
  handed off once and its hosts are set only with its value, so each
  entry's hosts are right at the hand-off (the table below).
  The mover reads each from its fixed place in the stopped agent and
  hands off only those it read; it skips the call when it read none.
  The roster's `has` doesn't decide it, because the mover's fleet can't
  know which an old agent holds (FIN-498); `has` stays on the wire,
  kept as it came, and nothing here reads it. Phase 2 names none (no
  one said Yes): its secrets stay in the old host's backup.
- **When.** Once the mover stopped the old agent, before it stages:
  the route takes only the old agent's current `stop_and_stage`, the one
  `command` names (409 otherwise), any of the kinds it asks for (400
  otherwise), while it is asked or staging (409). Each kind once, a
  value at most `HANDOFF_VALUE_MAX_BYTES` (4 KiB).
- **Where.** The router reads the values; `Migrations` sees only their
  kinds, and says whose Vault and agent they are for. Each value goes
  from the router straight into the person's Vault (`vault::hand_off`),
  in the call, and is never stored, logged or echoed on the way; a body
  that does not read is refused in words of its own. The mover reads
  them in memory (`finite-mover-host secrets`), never on disk or in a
  log.
- **Once.** The Vault keeps a receipt for each old agent and kind
  (`<old> <kind>`, its `receipts` table), in the turn that puts the
  secret. A hand-off whose receipt is there puts nothing: a retry, one
  cut short and sent again, or two at once never put back a secret its
  person removed, moved into a connector or set since. The receipt
  outlives the secret; a wipe empties it.
- **The entries** (`fragment_core::migrations::vault_secret`), each
  bound to the new agent's fragment, named `<kind>-<fragment>`, or, when
  that is over 32 bytes, `<kind>-<label…>-<hash>--<suffix>` (the label
  cut short beside 6 hex of the fragment name's SHA-256). A hand-off
  never takes the name of a secret bound to other agents (the Vault
  refuses it, 409):

  | Kind | Hosts | Place |
  |---|---|---|
  | Telegram's bot token | `api.telegram.org` | the default (Settings moves it into the agent's Telegram connector, `fromVault`) |
  | OpenRouter's key | `openrouter.ai` | `Authorization: Bearer` |
  | The Nostr key, as 64 hex | `finite.site` | `{"sign": "nip98"}`: the Vault signs the new agent's calls to finite.site's API and git remotes, so it brings the old agent's sites over itself; the key never leaves |

- **Answers.** `HandedOff {handed}`: the kinds the Vault holds a
  receipt for. One it refuses (its rules, a full Vault, another agent's
  name) is left out, and stays on the old platform. A Vault that could
  not be asked fails the call, and the mover tries again. The same
  hand-off again answers the same and puts nothing; once the stage is
  staged, its command is done and a hand-off is 409.

## A new agent's stage

The stage's id is `<old>.<phase>.<attempt>` (`stage_id`). It names the
move in the routine that starts the import.

| Route | Answer | Rules |
|---|---|---|
| `POST /api/migrations/stages/{stage}/open` | `StageOpened {stage, manifest}` | The manifest as the mover wrote it. |
| `GET /api/migrations/stages/{stage}/zip` | the zip's bytes | `Range: bytes=…` is answered 206 with that range alone. |
| `POST /api/migrations/stages/{stage}/copy?into=&offset=&bytes=` | → `StageCopied {sha256, bytes, crc32}` | One range of the zip (a file's bytes, or a part of them: 1 to the caller's upload limit) copied on the platform into the blobs of the fragment `into`, never through the agent's computer: hashed as R2 reads it (its CRC-32 too, which the caller checks against the zip entry's own before it takes the copy for the file), then read again into `into`'s blob upload as the agent, or for its owner (`for`), which the fragment checks as its client's (editor, and the hash). The same range again is the same blob. A range past the zip is 400. |
| `POST /api/migrations/stages/{stage}/done` | `StageDone {ok, report_sha256?, why?}` → `MoveAt` | Harmless either way. `ok` moves it to `retiring` with no command: its old agent stays stopped until staff's `retire`, and its person reads it as done. Not ok holds it for staff. The same `ok` again changes nothing. Either way the stage's routes refuse at once, and its objects are kept to their keep (Deletion). |

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
- **The manifest says it carries one** (`simplex: true`), so its
  person's comped seat is made always-on as it completes (The comp, "SimpleX
  needs always-on").
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
                  └── Yes{agent} ──► asked ──stopped──► staging ──staged──► importing ──done ok──► retiring, waiting ──retire──► retiring ──retired──► moved
                                       │                  │                   │  done !ok            │ retry       → asked (attempt+1)  │
                                       │                  │                   │                      │ restart-old → open               │
                                       └── deadline, or failed ×3 ────────────┴────────► held ◄──────────────────────────────────────────┘
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
| importing → retiring, waiting | the agent's `done ok` | 24 hours | held, `import_timed_out` |
| retiring, waiting → retiring | staff's `retire` (its command then) | none: it waits for staff | nothing |
| retiring → moved | the mover's `retired` | 24 hours from staff's `retire` | held, `retire_timed_out` |
| a staged attempt's objects | the alarm | 14 days after `staged` | deleted (the old host's backup remains) |
| archived → expired | the alarm | cutoff + 90 days | deleted |
| a phase 2 stage, its move ended any other way | the alarm | cutoff + 90 days | deleted |

A stage's `failed` report counts as an attempt, as a stall does. A
retirement's `failed` keeps its command for the mover to try again: the
moved mark it ends in is what finite.computer redirects by, so only its
day's deadline holds it for staff.

**Waiting for staff.** A good import is `retiring` with no command: its
old agent stopped, not retired, with no deadline but its stage's keep.
Its person reads it as done ("Upgraded.") and has their new agent. The `done`
comes from an agent following a skill, so nothing irreversible follows
it: staff check the import, then `retire` (the mover told to retire it
and mark it moved, its day from then), `retry` (a new attempt into the
same new agent, as after a held import), or `restart-old` (the old agent
runs again, the offer is open again). Until staff retire it, its old
agent has no moved mark, so finite.computer does not redirect its person
yet. Staff find them in `fragment operator moves --json`: `retiring`
with no `command`.

The release before this one reads such a row as a `retiring` move whose
command is lost: its staff ops refuse it, and past its stage's keep its
`retry` would retire the old agent. So the release that writes it is
forward only (Migrations step 7, docs/MODEL.md "Schemas"): the object
refuses the older code, and a failed deploy is fixed forward, never
rolled back across it.

Staff act on a held move: `retry` (a new attempt; a held phase 2 import,
its kept stage again, while it is kept), `restart-old` (the old agent
runs again, the offer is open again), `retire` (the import went well
enough by hand), and `withdraw` (out of the move).

## Deletion

| When | What |
|---|---|
| `done`, either way, and staff's `retire` | nothing: its routes refuse at once, and its objects are kept to their keep, for staff's retry or restart |
| 14 days after `staged` (phase 1, done or not, retired or not) | that attempt's prefix, from the alarm, at most 1,000 objects a pass |
| a superseded attempt (a retry, `restart-old`, `withdraw`) | its prefix, and its unfinished upload aborted |
| Never | nothing: Never only stops the asking |
| 90 days after the cutoff (phase 2), whatever its import did | its prefix. A phase 2 stage is the only copy left (its old agent was retired as archived), so it is kept until then: after a good import as after a failed one (for staff's retry). Its `done` ends only its reads |
| Backstop | the bucket's lifecycle rules: `p1/` after 16 days, `p2/` after 100, and incomplete multipart uploads after 1 |

A stage is never downloaded: the importer reads it in place, by ranges
(`fragment home … stage:<stage>`), only what it imports, so neither its
computer's disk nor its `/data` saves (docs/computers.md) ever hold it.
An old agent's home can be larger than a new computer's whole disk.

Nothing is left behind (Paul, 11-19). Beside what the import brings,
`fragment home files --store` keeps every other file of the stage whole
in the agent's storage (its owner's members-only fragment, the one
`fragment offload` moves into), each large file copied there by the
platform (`…/copy`), never through the computer, with a note where each
was that `fragment offload --back` brings back. Only a file whose name
says it may hold a password or key stays behind, unread, as before. Kept
blobs bill the owner's ledger as any fragment's storage does (R2's
price, with the margin: about $0.02 a GB a month).

## The operators' routes

`/api/admin/migrations`, from the CLI: the whole admin surface. There
is no page, no Glance column, and nothing is pushed anywhere else:
staff read a move's state and act on it here.

| Route | Answer |
|---|---|
| `GET /api/admin/migrations[?after=<old>]` | `AdminMoves`: each move by old id, a page at a time, with staff's `why` on each they withdrew |
| `POST /api/admin/migrations/{old}` `AdminAct {op, why?}` | `AdminMove`, after `op`: `retry` (a held move, or a good import waiting for staff, asked again as a new attempt; 409 when that would import into an agent another move is bound to), `restart-old` (the old agent started again, the move offered again), `retire` (the old agent retired as migrated: a good import waiting for staff's word, or a held move), `withdraw` with `why` (the move out of the roster's hands: offered no more), or `include` (staff's withdrawal taken back); `null` once the move is gone |

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

## What the next migration keeps

A team's Brain came over from finite.computer one teammate's move at a
time (FIN-774: the `finite-computer` profile's "Brains, as brains", and
`finite-computer.brains.py` beside it). Part of that is how any team's
move must work, and part is only how the old Finite kept its brains.
This section says which is which, so the next migration starts from the
rules, not from the old Finite's files. Two are in sight: a Hermes agent
from somewhere else moving onto fragment (one person), and a team moving
from finite.chat to a self-hosted box (fragment to fragment). It outlives
the move: the debt ledger's "The finite.computer move" removes the
profile only after the general rules are taken out of it.

### General: any team's move needs these

Each holds for any source. Today they are written in the
`finite-computer` profile and its helper. Taking them out is later work:
into `import-agent-home`'s `SKILL.md`, and a helper (or a `fragment home`
command, FIN-774's trade-off) that takes its versions and its base from
the source.

- **One home per team Brain.** Only the owner who runs the team makes
  it, after being asked whether someone already did. Every other
  teammate's import catches that one up: two would split what the team
  writes.
- **Never wider than the source.** What only some of a team could read
  never reaches everyone. Before anyone is invited, the owner is asked
  which parts only some should read. Those stay out (each can be a Brain
  of its own), and nothing is copied from a narrower Brain into a wider
  one.
- **Catch up, never copy over.** A later teammate's import decides each
  page from three versions: what the Brain took last (the base), the
  source's, and the Brain's now.
  - Changed only at the source: taken.
  - Changed only here: kept.
  - Changed on both: listed for a person.
  - Deleted at the source: listed, never deleted.
- **Newer by the source's own versions, never by bytes.** An import
  rewrites what it brings (an asset's note), so two copies of one
  unchanged page differ.
- **Only text the source published.** A page an old agent edited and
  never synced is listed, never taken: no version of the source held it.
- **An attachment goes by the notes that name it,** never by where it
  sits. It stays out when a note of a part kept back names it, and is
  recorded as kept out the moment any copy shows that, so no later
  import brings it in. Each attachment has one name, the same from
  every note that names it and in the record (`attachment_of`).
- **A reference copy until settled.** A teammate's copy goes only when
  the Brain holds everything in it, or a person has decided.
- **Only what sync carries, written whole.** A page `fragment sync`
  never carries (`syncable`, `valid_repo_path`) is refused, never
  written where it would look settled. A page is written beside its
  place and moved over it, so a failed write leaves what was there.

Two gaps hold for any version of it: two teammates catching up within
the same few seconds, and a page deleted between a catch-up's pull and
its push (the debt ledger's "A team brain's catch-up keeps its base in
a file").

### Only the old Finite's

| What | Why it is the old Finite's | A Hermes from elsewhere | finite.chat to self-hosted |
|---|---|---|---|
| `imported-from-finite.json`: the base as a file (each page's revision and what was written, the shared folders and the newest copy of each, the withheld attachments) | An agent can't read a Brain's history yet (FIN-780). | Nothing: one person's Brain is made once, and no teammate catches it up. | The Brain's history: the import's commit is the base, once the CLI reads a file at a commit and `main`'s messages (FIN-780). The withheld list becomes a fact of the Brain's own. |
| The stage's `versions` (FIN-773): each page's revision and hash, the Brain's latest sequence, each folder's identity and whether the old agent could read it | The old Brain's sync kept them; nothing else says which teammate's copy is newer. | Nothing: no copies to rank. | Git: a commit's ancestry says which copy is newer, and a page's path is its identity. |
| `copied`: which staged pages were their revision's exact text | A stage is a working tree, which may hold edits the old Brain never got. | Nothing. | Nothing: a commit holds exact text. |
| `/data/<path>` notes rewritten to `raw/assets/<path>`, with `finite_asset` | The old computer kept a note's file outside the Brain. | Its own layout's mapping, if its notes name files; the one-name rule stays. | Nothing: attachments already sit in `raw/assets/`. |
| Folders by identity and readability: one left out, one the old agent couldn't read, another at a shared folder's path | The old Brain shared folders to some of a team; here a Brain is shared whole. | Nothing: all of it is the person's. | Nothing: a part only some may read is already a Brain of its own. Each Brain's members are invited again. |
| `owner` and `kind` in the name file decide make or reference copy | The old agent kept a working copy of every Brain it could read. | Nothing: all of it is theirs. | The fragment's own owner and members. |
| The old Brain's own files left out (`AGENTS.md`, `_index.md`, `.keep`, `_wiki/`), and the push's top-level check | The old Brain's tree. | Its layout's. | Nothing: the repo is the Brain. |

`finite-computer.brains.py` is both. Its comparison, withholding,
refusals and whole writes are the general rules above. Its inputs are
the old Finite's: the record file, the stage's `versions`, the name
file, and `/data`. Taken out, it reads them through the source.

A team on finite.chat has no copy per teammate: a Brain is one fragment
its members share. A move to a self-hosted box moves that fragment once,
with its history. A teammate-at-a-time catch-up comes back only if the
team keeps writing on finite.chat after the move: then the newer commits
come over as a merge, with the move's commit as the base.
