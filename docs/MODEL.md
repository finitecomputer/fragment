# MODEL — the fragment core model

The shape the cell is built to: decided 2026-09-23 (the five changes
Paul approved), on Cloudflare since phase 2 of docs/cloudflare-v1.md.
Every mechanism names the Cloudflare primitive it uses; docs/api.md is
the wire contract in full.

## The five changes

1. **Files in git, state in SQL, history in channels.** Every file is in
   git (code.storage): code, templates, documents, uploads, generated
   images and video. git is the one file tree, its history, and the sync
   to computers and local folders. A file of 1 MiB or more is stored in
   git as a pointer (hash, size, type) whose bytes live in R2 under that
   hash; syncing resolves pointers, so computers and agents see real
   files. App state lives in SQL. What happened lives in channels.
   File-based apps (a vault, a wiki) stay first-class: for them the files
   *are* the state, and operations read and write them through
   `this.files`, `call.files`, and `job.files` (reads at `main`, the
   working copy).
2. **One execution primitive: the operation.** Named, schema-typed,
   role-checked, idempotent by operation id, ledgered. Every trigger
   (browser, CLI, agent tool call, cron, webhook, channel message, file
   change) is a way to invoke an operation.
3. **One log: channels.** Append-only, ordered, paged, per fragment. The
   audit trail, the inbox, room messages, and chat transcripts are all
   channels. Presence stays ephemeral.
4. **Membership is live cell state; every actor is an identity.** People
   and agents are identities (npubs: docs/cloudflare-v1.md, decision 45)
   holding keys, resolved live by the registry on every signed request
   whose answer depends on who is asking (a page everyone who may see it
   gets alike asks nothing). Grants,
   invites, and revocations are transactional and take effect on the
   next request. Each fragment is its own browser origin.
5. **Agents are fragments a computer runs** (docs/cloudflare-v1.md,
   decisions 13 to 15), members of the fragments they work in. No
   fragment declares one (Agents and Computers, below).

## Anatomy of a fragment, in Cloudflare terms

Connection authorization's pending state lives in the Registry's
`connection_signins`: a state hash, session hash, provider, issuer/subject,
configuration hash, sealed verifier and expiry. A browser cookie proves the
round trip; a one-use transaction is consumed before the provider exchange.
Completed connection credentials live only in WorkOS Pipes. See
[browser-bound Pipes connections](pipes-browser-binding.md).

| Part | Cloudflare primitive | Holds |
|---|---|---|
| Supervisor | a Durable Object (`Fragment`), platform code in Rust | members, invites, the audit copy of the operation ledger, channels, schedules, wrapped secrets, the file-plane pins and tree index, the live code pin |
| App | a **Durable Object Facet** named `app@<incarnation>` (each life of a fragment's name has its own; `app` for fragments made before 2026-09-27), started through the **Worker Loader** at `live@SHA` from `platform.js` (platform code) wrapping the author's `App` class | the author's SQLite database (`this.ctx.storage.sql`), which the supervisor's tables never share, and the mutation ledger `_fragment_ops` beside it |
| Files | code.storage git (wire contract unchanged) | every file; one of 1 MiB or more as a pointer to its blob |
| Blobs | an **R2** bucket (`BLOBS`) | the bytes of large files, content-addressed by SHA-256; only blobs a pointer at a branch tip references are kept |
| Jobs | **Workflows** (one instance per run) | multi-step or long operations |
| Deliveries | **Queues** (at-least-once, dead-letter queue, `message.id` as idempotency key) | records to channel subscribers, and web push |
| Schedules | the supervisor's alarm (the platform takes the app's away) | per-fragment cron and retry backoff, multiplexed onto one alarm |

The supervisor is on the path of every call into the app: it checks the
caller's role, validates input, applies quotas, and ledgers the result
before the app code runs or its result leaves. The app's code is loaded
with an env the platform builds, holding one capability (`FILES`, a
`ctx.exports.Files({ props })` bound to its fragment), and
`globalOutbound: null`: it has no network, and a job reaches out only
through its `job.fetch` steps, which the supervisor takes.
`WorkerCode.limits` bounds `cpuMs` and `subRequests` per invocation.

Why a facet and not our own SQL loopback: a facet gives author code a
real, private SQLite database that cannot see platform tables
(finite-next filtered SQL strings with a regex instead). A facet is a
database of its own, so the supervisor's transaction never encloses a
facet call, and atomicity lives inside the facet: platform code runs
each mutation and its ledger row in the facet's own `transactionSync`.

## Operations

Declared in `fragment.json`:

```json
{
  "operations": {
    "list":     { "kind": "query",    "role": "viewer", "input": { "type": "object", "properties": {} } },
    "add_todo": { "kind": "mutation", "role": "editor", "input": { "type": "object", "required": ["text"], "properties": { "text": { "type": "string", "maxLength": 500 } } } },
    "digest":   { "kind": "job",      "role": "editor", "input": { "type": "object" } }
  }
}
```

Implemented as methods of the author's `App` class (the facet answers
RPC methods), each receiving `(input, call)`, where `call` names the
caller (`principal`, `role`) and, in a mutation, collects its effects
(`call.publish(channel, body, kind)`):

- **query** — read-only; not ledgered; may be async and may call read
  capabilities (files, channels, blobs); may be re-run on change signals
  to drive live views.
- **mutation** — synchronous, over the app's own SQL only: no `await`, no
  capabilities, no network. Platform code in the facet runs it and its
  ledger row in one `transactionSync` keyed by (principal, operation id).
  What it wants to happen next (channel records now; notifications,
  starting a job, a vault's file writes in later slices) it declares as
  **effects** through `call`; the ledger row keeps them, so the ledger is
  also an outbox. Ledger rows are kept seven days: a replay after that
  runs again.
  An **ephemeral** mutation (`"ephemeral": true`, for a "latest value"
  write such as a screen's frame) keeps no ledger row, which is a few
  hundred bytes of the app's 16 MiB a call. It keeps: its writes and its
  answer in one `transactionSync` (a throw rolls them back), the role and
  schema checks, the database's cap, and the change signal to live
  views. It gives up: the replay (the same id runs again, with any input:
  a retried call, job step, or triggered run may run it twice), its
  effects (their outbox is the ledger row, so publishing, pushing, or
  writing files rolls it back, 422), the `ops` record, and the pending
  row: nothing is left to settle after a crash (its writes committed or
  did not).
- **job** — runs as a Workflow instance, one per run; its method gets
  `(input, job)`, and each `await job.call(op)` (a query, a mutation, or
  another job), `job.fetch(url)` (the external effect: secrets are added
  at the egress point), `job.publish`, or `job.sleep` is a durable step,
  so retries, backoff, and sleeps are the Workflow's. The body runs in
  the app facet by replay up to its next step; the Workflow, platform
  code, takes the step through the supervisor, which keeps each step's
  answer (the replay reads them from there, and a step retried after its
  answer was lost is not taken twice). fragment's failure leg (held
  runs, replay, auto-pause, the hop budget) is on runs (`docs/api.md`,
  Jobs and triggers). `waitForEvent` (approvals, invites) comes with
  agents.

The call path, for every trigger:

```
trigger ─▶ principal + op id + input ─▶ Fragment (supervisor)
   role check ─▶ schema check ─▶ pending row (principal, op, depth; its seq is the run)
   ─▶ App facet __mutate(id, name, sha(input), input, run)
      facet transactionSync: ledger lookup (a week's window)
        ├─ same id, same input      → the stored result and run (a replay)
        ├─ same id, different input → reject (conflicting body)
        └─ new                      → the author's method, then its ledger row with the run
   ─▶ the pending run's effects, checked in Rust, applied keyed by (id#run, index)
   ─▶ `ops` record + pending row dropped ─▶ change signal to subscribers
```

A supervisor that dies after the facet committed loses nothing: its
pending row outlives it, and on activation the supervisor asks the facet
about each pending id and applies the run it recorded (or drops the row
when the facet never committed that run). Who called, and what, are the
pending row's facts, never the app's: the app can write its own ledger
table, so a row it writes there is never applied. A replay applies
nothing again, since only a pending run is applied; a refused effect
settles its run for good, so it cannot block the app; a passing failure
leaves the run pending for a later try (docs/api.md, Apps).

Triggers: an HTTP call from the UI (`POST /__op/<name>`), `fragment call`
from the CLI, an agent tool call (an operation's schema *is* its tool
schema), a cron entry, an inbox webhook, a channel message, and a pin
move (file change). Cron entries, channel messages, and pin moves are
declared in `fragment.json`'s `triggers` and start runs as the fragment's own key;
a triggered mutation is a run of one step, so it retries and is held
like a job. A custom `App.fetch` stays
available for routes that are not operations (dynamic HTML, file
serving), but agents and the platform only use operations.

## Channels

A channel is a supervisor table: `(channel, seq)` → `{at, principal,
kind, body, op_id}`, append-only, with a per-channel retention policy.

- Built in: `events` (audit, platform-written), `inbox` (webhooks,
  appended by the inbox route; its pending records are the runs they
  triggered that have not succeeded, and at 1000 a post is 429), `ops`
  (`{op, id}` per applied
  mutation: the ledger's public view, and the marker that its effects
  were applied), and app-declared channels (a chat transcript, a room's
  messages), declared in `fragment.json` with the role that may read
  them. Clients never append: records come from the platform and from
  mutations' effects, except on a channel the fragment declares
  postable, where the platform appends a member's record for them
  (docs/cloudflare-v1.md, R18); one that says `"signedIn": true` takes posts
  from signed-in posters only (an anonymous visitor holding the role is
  refused).
- Subscribers: hibernatable WebSockets that resume from a cursor (a
  WebSocket closes when the cell moves, so clients reconnect with their
  last `seq`); channel-triggered operations; agents; and outbound
  deliveries (a subscription's URL, web push) through a Queue.
- Presence is per socket and never stored.
- A channel may declare record kinds its members' lists show
  (`listed`): each member's newest record of those kinds rides their
  row in each member's list whose role may read the channel, so a list
  says what a fragment's people and agents are doing (a chat's agents:
  working, waiting) with no fragment read (docs/api.md, Listed records).
- fragment's per-room persisted document becomes a query operation plus
  a change signal; `ctx.state` becomes the app's SQL.

## Principals and membership

- A principal is an identity: a person, an agent, or a fragment, with
  one or more public keys in the registry (finite.computer's BANKS
  model, docs/cloudflare-v1.md, R15). The CLI proves a key with NIP-98 and the
  registry names its identity; a browser has a platform session that
  maps to the person; an agent signs with its cell's key; a fragment
  with its own key (unregistered: it is the principal of its own
  triggered runs only). The platform holds no person's private
  key. An agent's designated owner reads what the agent can read, as a
  viewer, and never acts through it (docs/api.md, Principals and access).
- Members and invites are supervisor tables. A grant or revoke is one
  transaction; a revoke closes that principal's sockets. The `events`
  channel records every change. Membership leaves `fragment.json` (a git
  commit can no longer grant access). Only the owner manages members,
  invites, visibility, and tokens; a member may leave. Members name
  identities, so replacing a key rewrites no grant. Each identity's list
  of fragments is an index in its own `Principal` cell, fed from the
  fragment's outbox. A request sends at most one round of the outbox (32
  lists at once, its own change first) and the alarm the rest, so no
  request waits on a list per member. A delete ends the life at once and leaves its
  members' lists, the app's database and the blobs to the alarm
  (cell `ended.rs`).
- Visibility: `public`, `link` (a token that is a capability), or
  `members`. **A public fragment is a website anyone can use, writes
  included** (a public chat, a guestbook): an operation may declare
  `"role": "public"`, callable by anyone who can see the fragment. An
  anonymous visitor gets an ephemeral principal (a random value held in
  an HttpOnly cookie on the fragment's origin; the principal is `anon:`
  plus its hash), so their writes are attributed and rate-limited like
  anyone else's. On a `link` or `public` fragment the link holder counts
  as a viewer; the link is its owner's to give, and a fragment opened
  again from `members` gets a new one. A fragment is made `link`, a chat
  or an agent `members` (2026-10-13). A member's share is theirs and
  their agents' (Paul, 2026-10-13), each agent capped (R17). Operation
  ids belong to their caller: the ledger keys a mutation by principal
  and id.
- Origins: each fragment is served from
  `<name>.fragment.boats`; the platform (login, the share
  sheet) from `fragment.club`, another site
  (docs/cloudflare-v1.md, decision 5). The router checks the hostname against the configured
  suffix before it trusts it.

## Agents

An agent is a fragment (kind `agent`) with an identity of its own, owned
by a person and run by a computer of theirs: today a Hermes profile,
whose bridge turns the records of the channels it follows into turns
(docs/cloudflare-v1.md, decisions 14 and 15; docs/computers.md;
docs/chat-records.md). It acts on fragments through the platform's API
as their member, for whoever asked: every call names them (`for`), and
acts with the lower of their role and the agent's cap
(docs/cloudflare-v1.md, R17). Acting for its owner it also starts and
stops its owner's other agents on their computer, so a person's default
agent keeps the rest (decision 62, proposed). The in-fragment goose agent (a loop in a Durable Object of its own,
a fragment's `agent` block, `job.agent`) went on 2026-10-07 (issue
#156): one runtime, with one set of turn semantics.

## Computers

A computer is a container a person owns, run by the `Computer` Durable
Object, which starts and stops it, saves and restores its `/data`, and
runs its egress. Its image runs the turns of the person's agent
fragments, as those agents; the platform names no agent runtime. The
contract is docs/computers.md.

## Where each fact lives

Every change is checked against this table.

| Thing | Source of truth | Copies must be |
|---|---|---|
| File bytes, their history, `main` and `live` | code.storage git | a local folder is a disposable working copy |
| Tree index (path, size, SHA per pinned commit) | derived from git | in the cell's SQLite; names its pinned SHA; moved by the platform's own moves (its commits, a deploy), a writer's refresh after its push, or the poll backstop |
| Manifest and declared operations | `fragment.json` in git | the cell's copy at the pin; an invalid one at a new pin keeps the last good and says why (`status.code.error`) |
| Members, roles, invites | the fragment's supervisor | grants and revokes are transactional; `events` records each. The registry keeps each invite waiting on an email against that email, to meet it at a sign-in: a pointer the fragment overrules (one with none waiting is forgotten); a claim the fragment could not take is tried again by the registry's alarm, bounded, then waits on the next sign-in |
| Cell state (supervisor tables, the operation ledger, channels, the app's SQL) | the Durable Objects' own storage | none |
| A platform object's schema: its tables, and the steps that made them | its class's steps, `crates/core/migrations/<class>/` (Schemas, below) | each object's `_schema` holds the steps it took (number, name, SHA, ran or adopted) and refuses code whose steps differ (`schema_refused`); `crates/core/migrations/schema.lock` locks each step and what the steps make, and `cargo xtask check` holds the tree to it. An app facet's SQLite is its author's |
| The release a call between the Worker and an object (or two objects) was made by | the caller's `FRAGMENT_DEPLOY_ID`, its deploy's commit (Calls between releases, below) | stamped on each call (`x-fragment-release`) and checked first by the object, which refuses another's (503 `deploying`) before it acts; never stored |
| The release a browser's page of the platform runs, and the one under it now | the shell's page as it was answered (its `<meta name="fragment-release">`, filled with the answering `FRAGMENT_DEPLOY_ID`), and every answer of the platform's host (`x-fragment-deploy`) | the page compares the two on each answer it reads and reloads once they differ, at a safe moment (docs/api.md, The shell, "A new release"); its tab keeps its last such reload (`sessionStorage`: from, to, when, how often), so a deploy's old side answering again, or a reload that met it, is no loop; it reloads only while hidden or at the person's next move, and while no chat's page says it is busy |
| A chat's unsent words (its composer's draft) | the chat's page, as its person types | that page's origin's storage (`localStorage`, `fragment.chat.draft.v1`), as the principal it is signed in as: put back for that principal alone as the page loads (anyone else's let go), emptied as it is sent, and let go when the shell asks as its person signs out or uses another account (docs/chat-records.md, "Its draft, kept") |
| Large file bytes (1 MiB or more) | R2 (`BLOBS`), keyed by SHA-256 | git holds a pointer; a sync resolves it; a blob no branch tip references is deleted |
| Identities (npubs) and their keys, each person's own key (sealed), sign-ins and their verified emails, agents' owners, sessions | the registry (fragment's BANKS: docs/cloudflare-v1.md, decisions 45 to 50) | an identity's npub never changes; an email names at most one person; sessions and caches name an identity and never outlive a revocation; cookies hold only tokens, the registry their hashes |
| A person's wipe: how far it got, and that it locks them | the registry's `wipes` (docs/api.md, Operators) | a wiped person's ledger, list and computer each keep one row saying so, and take nothing more |
| Secrets | the Durable Object that owns each, sealed for it; the deployment's own in its Secrets Store (docs/secrets.md) | never in a repo, a log, a command line, or an app's env |
| Orgs, their admins and seats (held by an npub, or pending on an email; comped or paid) | the registry (docs/billing.md) | a seat holder's plan and standing on their ledger, and their computer's always-on, are pushed from it: one queued row a person, pushed one at a time by its alarm, each read fresh as it goes, so the newest push is of the newest state. The ledger keeps the seat in the registry's order (`SetSeat.seq`), refusing an older one as stale and naming its own, which the registry's next order passes; always-on carries no order (the one row a person orders it), and a computer made later asks for a push as it is made, its making failing when it cannot. The registry is the seat's one writer: operators comp and end seats through it (the ledger's `seat` command went 2026-10-16); an operator's `plan` command is the debt ledger's |
| Whether a person is a guest (decision 49) | their seat in the registry and, holding none, their ledger's plan (`fragment_core::org::guest`) | `GET /api/seat`'s `guest`, decided at each ask: the shell, its Billing and its welcome read it alone |
| What is sold: a seat's and a pack's price, the credit a seat includes | core's constants (`fragment_core::stripe`, `ledger::included`) | Stripe's Prices, which Checkout refuses at any other amount; every page shows them from `GET /api/seat`'s `offer` |
| A subscription's status, items and periods; an org's Stripe customer | Stripe (docs/billing.md) | the registry's copy (`orgs`) is written only from a subscription fetched from Stripe (a Checkout's return, a webhook, the daily reconcile), never from an event's own copy; a newer subscription replaces one only once it has ended, and an older event changes nothing |
| The image a computer's next start runs | the deployment's container config: each image name's reference in the Worker version (`ctx.container.images`); a computer's pin names one | the Computer DO's last look (`Images`), at its every event and every read of its view; a start runs what it looked at and records it as the image that life runs, from which its owner is told an update is ready |
| An agent's name and picture | the agent fragment at live: `fragment.json`'s `meta.title` and its `avatar.png` (docs/api.md, "An agent's name and picture") | its cell's face, read at each install of live (a picture checked, a bad one refusing the install); its members' lists, `__people`, `job.people`, the share sheet and the export read that face, its picture served from the commit the install checked; its computer's profile (Hermes' `profile.yaml`, and its config's `agent.system_prompt`, which tells the agent its name at every turn), rewritten from it within seconds while awake and at each boot; its screen's page and window, told it by the shell as they open and on a rename (its computer knows the agent by its label alone). No chat's title and no `SOUL.md` copies it. Its label, the word that `@mentions` it, never changes with it |
| A fragment's scheduled records (data schedules: at each minute a cron names, a record on one of its channels; an agent's routines are these, on its `tasks`) | the fragment's supervisor (`record_schedules`: docs/api.md, "Scheduled records"), beside its code's cron schedules (`schedules`, made again from the installed manifest at each install), which never touch them | each fire's record on its channel, keyed by the schedule and its minute, so appended once; its owner's export copies them. An agent's computer keeps none (Hermes' own cron is off: decision 38), so no restart, upgrade or sleep of it loses one |
| An agent's colour | its identity, through one function (templates/agent/site/agent-color.js, which the chat template and the shell link to) | none: never stored (`agent.json` names none) |
| An agent's model (its tier, and a model of a provider its owner connected: `model: {provider, id}`) | the agent fragment's `agent.json` in git (docs/computers.md, "An agent's own model") | its computer's profile config, rewritten from it within seconds while awake and at each boot; used only while the agent's credentials hold that provider (else its tier), which its owner's view of the computer names (`modelProviders`); the agent's page shows the model it runs from these and `agent.json`, as its owner's shell hands them over |
| The model route's models (each slot's model, and where it runs) | the deployment's config's `models` (`FRAGMENT_MODELS`, a Workers variable; docs/computers.md, Models) | their ids only, by slot, in each computer's guest view and its owner's view (`models`; Settings and the agent's page show them), which our Hermes image reads every 3 s and writes into each profile's config; `GET /v1/models` and the route's refusal of an unknown id list the same ids, read from the config at each request |
| A computer's agents (which agent fragments it runs) | the Computer DO's `agents`, assigned and unassigned by its owner (docs/computers.md) | the agent fragment's own `computer` (whom it signs for) and the registry's name of its agent (`agent_fragments`); they follow the fragment: its delete unassigns it from its computer and drops the registry's row, that life's agent alone, retried until done (cell/src/ended.rs) |
| Money | each payer's ledger (docs/ledger.md) | meters batch usage rows to it, idempotently |
| A job's paid AI answer (text, decision, image), including a rejected decision answer | the fragment supervisor's `paid` receipts, keyed by incarnation, run and step (docs/api.md, AI) | retries and replays settle and reuse the receipt; no provider call once a receipt exists |
| Audit trail | the `events` channel | pin moves recorded as events |
| The agent docs | `cli/SKILL.md` and `cli/GUIDE.md`, naming no host | never edited elsewhere: compiled into the CLI (`fragment skill`, `fragment guide`, filled with its host), and the release's files on the platform (`/llms.txt`, `/llms-full.txt`, filled with its own platform and fragments as served; docs/api.md, Agent docs) |
| The release's files: the templates, the shell's files, the agent docs | the repo (`templates/`, `cell/shell/`, `cli/`) | the cell's index of them (path, SHA-256, size, each template's release), made by its build; their bytes in the Worker's Static Assets, named by hash, written by the same build and uploaded with it, served only through the router (docs/api.md, The release's files); the CLI embeds the templates |
| A fragment's preview card, and what its page reported as the card's shot loaded it | the fragment's own storage (`fragment_core::card::Cards`; docs/api.md, Cards) | status's `page` reads it; a report with errors is also a `page.errors` event |
| What a fragment's members are doing: each one's newest record of a kind a channel lists | the channel's records | the fragment's `listed` (written in each record's append while its kind is listed, what is listed no more dropped at an install, a member's going when they leave), and each member's list row, sent from the index outbox as the row's face is (at most once in 3 s), made as it is sent from what is installed then and carrying only the channels its role reads (`channel_read_role`); a list's copy is at most a minute behind while its poster keeps at it (docs/api.md, Listed records) |
| A person's own view of their fragments: what they archived, and how far they have seen each chat | their list, the `Principal` cell (principal.rs; docs/api.md, Control API) | none: no fragment holds it, and it goes when they leave the fragment |
| A person's export: whether it builds, how far it got, and its archive | their list, the `Principal` cell (its row, its job and its kept place: export.rs; docs/api.md, Export), and its archive in R2 (`exports/<npub>/<id>/archive.tar`) | the archive is a copy of what each fragment, the registry and their computer answered its person as it was read, never updated; it is deleted seven days after it is ready, when a newer export starts, or by a wipe of its person |

No file bytes persist in the cell's SQLite: a file lives in git or, at
1 MiB and above, in R2 under its hash, named by a pointer in git.

## Schemas: the platform objects' steps

Each platform Durable Object class (`Fragment`'s supervisor, `Registry`,
`Principal`, `Ledger`, `Computer`) declares its schema as numbered steps,
one SQL file each under `crates/core/migrations/<class>/`, run by
`fragment_core::migrate` as an object starts, in one transaction: a new
object takes them all, an older one the steps it lacks, and one that
fails rolls them all back. Step 1 is each class's schema as it stood on
2026-10-19; an object made before then (none holds a version) is adopted
at 1 only when what it holds is step 1's, text for text, or a shape the
class names with its fix (a fragment's `members.people_only`). An object
refuses its code (`schema_refused`, on every call, naming it and why)
when it holds a newer version (a rollback), another code's step of the
same number (another branch's), or tables from before versions that are
not step 1's: a reset is the operator's call, and nothing is guessed.
A wipe's or a delete's tables are dropped and made again as the object
holds them (`migrate::remake`), so they keep every step's shape.

What removes everything still runs on a refused object: a fragment's
delete (its cleanup, and the release of its storage after, which makes
the object current again), and a wipe's end of a fragment, a list, a
ledger or a computer (a wiped one answers as wiped). The Registry, the
deployment's one, refuses everything, its wipe steps included (those
delete a person's rows by name, and could miss a newer step's table):
if it ever refuses, its recovery is Paul's call, and no operator reset
exists for it. A refused object's alarm wakes again, at most every five
minutes, only where newer code could take it (a newer version, another
code's step, a step that failed), never for a reset required.

**No rollback below the versions (#383) once any class has a step 2.**
Code from before them ignores `_schema`, runs its step 1 as it always
did, and its wipes and deletes make tables again from that text,
dropping a later step's columns under a `_schema` that still names them.
A rollback to code that has versions is refused as above, object by
object, until it is rolled forward.

To change a schema, add a step: the file
`crates/core/migrations/<class>/<NNNN>_<name>.sql` (an `ALTER TABLE …
ADD COLUMN`, a new table or index, or a table rebuilt), its `step!` at
the end of the class's `steps` (crates/core/src/migrate.rs, whose
module doc says it in full), then `cargo xtask schema-lock`, and commit
the three. A step is never edited, renumbered or removed once merged;
`cargo xtask check` fails a schema changed any other way: the lock
against the steps, the lock against master's, step for step (CI names
master: a pull request's base, or what a push moved it from), and a
statement in the cell that makes, alters or drops a table
(xtask/src/guards.rs). The lock's SQLite refuses what a Durable
Object's does (PRAGMAs that write, a transaction's or a savepoint's
statements, ATTACH, TEMP objects), so no step that would fail there
locks; and each class's schema as the deployments ran it before
versions is checked in (`crates/core/migrations/deployed`) and must
adopt.

## Calls between releases

A deploy does not move the Worker and its Durable Objects at one
instant: for seconds (minutes, Cloudflare says, at worst) a Worker of
the new release calls objects still running the old one, an old Worker
calls objects already on the new one, and objects call each other
across the two. Their calls (the router's to a supervisor, any call to
the Registry, a fragment's to a list or a ledger, the queue consumers',
a run's Workflow's) are the platform's own and change shape freely
between releases, so each must meet its own release on the other side.
On 2026-10-20, as master `bc8b87e7` replaced `33e323cd` on finite.chat,
the new Worker asked the old Registry to exchange a sign-in's WorkOS
code (#351 had changed its answer's shape); the old Registry exchanged
it and answered the old shape, the new Worker could not read it (503
`registry_unavailable`), and the person's refresh replayed a code
already spent (WorkOS's 400).

So every call into a platform object carries its caller's release, and
the object admits only its own (`fragment_core::release`). The release
is the deployment's `FRAGMENT_DEPLOY_ID`: a Worker variable, so a part of
the Worker version, and the router, the queue consumers, the Workflow
and all five object classes are one script (cell/entry.mjs), each
running with the variables of the version it runs; one deploy is one
value everywhere. It is the deploy's commit (`git rev-parse --short=12`,
what `/healthz` answers in `x-fragment-deploy` and CD checks), so a
commit deployed again (a secret or a variable changed) is one release,
whose code agrees with itself; an uncommitted tree's is that deploy's
alone (`<sha>-dirty-<second>`). Dev and the e2e run one build in one
`workerd` (`dev` everywhere), and nothing changes for them.

- **The caller** stamps it (`x-fragment-release`) on every call: the
  cell calls an object only through `routed::send` and `send_again`, and
  the JavaScript only with requests `InternalRoute.request` makes (both
  held by xtask's guard).
- **The object** checks it first in its `fetch`, before it reads the
  call or does anything for it (its schema's steps ran as it started,
  its own release's, whoever called): a call from another release,
  newer or older, or one naming none (a caller from before this check),
  is answered 503 `deploying`, marked as that refusal
  (`x-fragment-release-refused`), and nothing was done. Its alarms and
  its sockets' messages are its own release's, and the calls they make
  are checked as any other.
- **On a refusal** the caller asks again, since nothing was done,
  however the call acts: after 250, 500 and 1000 ms, at most four asks
  (`release::ASKS`), then it answers `deploying` itself (the person
  tries again; the CLI reads it as `unavailable`). A request that cannot
  be made again (a body streamed through, a socket's upgrade) is asked
  once. An outbox, a queue message or a Workflow step that meets it is
  retried as any failure of theirs is. A `deploying` an object answers
  because a call of its own was refused is its answer, never asked
  again: it may have acted before that call.

Both directions refuse alike, as a release is a commit and commits are
not ordered: an old caller at a new object will not be admitted from
that isolate, and its asks run out; a new caller at an old object is
admitted once the object restarts on the new release. A deploy is all at
once (`wrangler deploy`): a gradual deployment, two versions serving for
a while, would refuse between them for that while. What outlives a
deploy is not a call and is not covered: a queue message or a stored row
written by one release is read by the next, so its shape stays readable
(a message no release can read is acked and logged; a table changes by
its steps, above). The first deploy of the check met callers that named
no release, refused like any other, and old objects that checked
nothing, as before it.

A browser's page outlives a deploy too, and it is no call: the shell (and
the chats' pages it frames, the release's own) runs the release it
loaded with for as long as its tab or its home screen's app stays open,
against whatever the platform runs now. So its page names the release
that answered it, every answer of the platform's host names its own
(`x-fragment-deploy`, what `/healthz` says), and the page reloads itself
once they differ, at a safe moment: never over anything unsent, and
never in a loop, the deploy's old side answering again included
(docs/api.md, The shell, "A new release"). Until it reloads it asks the
API as the release it loaded with did, as any older client does.

## Limits (initial; each enforced and tested)

| Limit | Value | Why |
|---|---|---|
| operation input | 256 KiB | a request, not an upload (uploads go to the file plane) |
| operation description | 1024 characters | what an agent reads to choose an operation: a sentence or a paragraph, not a manual |
| inline file in git | under 1 MiB | a file of 1 MiB or more is a pointer to a blob |
| operation or step result | 1 MiB | the Workflows step-result limit |
| channel record body | 64 KiB | records are messages, not files |
| channel page | 1000 records | bounded reads |
| app facet database | 16 MiB, per mutation | Paul's cap (large files belong in git storage); a write from elsewhere grows it, billed to its owner (docs/api.md, Apps) |
| `cpuMs` per invocation | 30 000 | a runaway loop cannot hold the fragment |
| `subRequests` per invocation | 50 | bounds fan-out |
| inbox pending | 1000 | overload is a 429, not memory pressure |
| hop depth | 16 | carried from fragment's loop guard |
| `public`-role calls | 60 per minute per anonymous principal, 600 per minute per fragment | public writes must not become an abuse amplifier; tunable per operation |
| a fragment's host label | 63 bytes: its name, `<label>--<suffix>`, and a branch's mark; every deployment leaves 39 for labels | one DNS label under one wildcard certificate; refused at create, never cut (docs/api.md, Names) |

## Answered (2026-09-23)

- **Authoring shape:** one `app.mjs` whose exported `App` class (a
  Durable Object class the supervisor starts as the app facet) has one
  method per operation, plus an optional `fetch` for custom routes.
- **Anonymous callers:** public fragments are websites; anonymous
  visitors may read and write through `public`-role operations, with an
  ephemeral principal each (above).
- **Channel retention:** `events` 90 days, `inbox` until acknowledged,
  app-declared channels forever (one people may post to, its newest
  10 000).

## Answered from the spikes (2026-09-23)

- **Mutations are synchronous and return their effects** (spike 2): a
  mutation cannot `await`, call a capability, or fetch; queries and jobs
  can. Paul: yes.
- **The app facet database is capped at 16 MiB.** Paul: large files
  belong in git storage, so a 16 MiB cap.
- **Every file is in git; large ones are pointers** (Paul, 2026-09-23).
  Files of 1 MiB or more are pointers in git with their bytes in R2
  (code.storage's HTTP API has no LFS path). Syncing a fragment to a
  computer or a folder delivers real files, generated media included.
  Only the latest version's bytes are kept: a blob no branch tip
  references is deleted. A future exception (keeping every version of,
  say, a large Photoshop file) is not needed now.
