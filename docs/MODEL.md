# MODEL — the fragment core model

The shape the cell is built to, in Cloudflare's primitives. docs/api.md is
the wire contract; docs/cloudflare-v1.md holds the decisions.

## The model

- **Files in git, state in SQL, history in channels.** Every file is in
  git (code.storage): code, documents, uploads, generated media. A file of
  `fragment_core::blob::BLOB_MIN_BYTES` or more is a pointer in git whose
  bytes live in R2 under their hash; a sync resolves pointers, so
  computers and agents see real files, and a blob no branch tip names is
  deleted. No file bytes persist in the cell's SQLite. App state lives in
  the app's SQL; for a file-based app (a vault, a wiki) the files are the
  state.
- **One execution primitive: the operation.** Named, schema-typed,
  role-checked, idempotent by principal and operation id. Every trigger
  (a page, the CLI, an agent, cron, a webhook, a channel record, a file
  change) invokes one. A query reads; a mutation is synchronous over the
  app's own SQL, and declares its effects, applied once it commits; a job
  is a Workflow of durable steps (docs/api.md, Apps; Jobs and triggers).
- **One log: channels.** Append-only, ordered and paged, per fragment:
  the audit trail (`events`), the inbox, a chat. Clients never append:
  records come from the platform and from mutations' effects, except on
  a postable channel (R18). Presence is per socket and never stored.
- **Membership is live cell state.** People, agents and fragments are
  identities (R15), resolved live by the registry whenever an answer
  depends on who asks. Members and invites are the fragment's own tables,
  never `fragment.json`, so no git commit grants access. A grant or a
  revoke is one transaction, in force at the next request.
- **Each fragment is its own browser origin**, on a site apart from the
  platform's (decision 5). The router checks a host against the
  configured suffix before it trusts it.
- **A public fragment is a website anyone can use, writes included,**
  through `public`-role operations; an anonymous visitor is an ephemeral
  principal (`anon:`), attributed and rate-limited like anyone else.
- **Agents are fragments a computer runs** (decisions 13 to 15), members
  of what they work in, acting for whoever asked (R17). A computer is a
  container a person owns, run by its `Computer` object; its image
  decides what runs (docs/computers.md).

| Part | Cloudflare primitive | Holds |
|---|---|---|
| Supervisor | a Durable Object (`Fragment`), Rust | members, invites, channels, schedules, sealed secrets, the file pins and tree index, the live code pin |
| App | a Durable Object Facet (`app@<incarnation>`), loaded by the Worker Loader from the live commit | the author's SQLite (capped at `APP_DB_MAX_BYTES`: large files belong in git) and its mutation ledger |
| Files | code.storage git | every file; a large one as a pointer |
| Blobs | R2 (`BLOBS`) | large files' bytes, by SHA-256 |
| Jobs | Workflows | one instance per run |
| Deliveries | Queues | records to subscribers, web push |
| Schedules | the supervisor's alarm (the app gets none) | cron and retries |

Why a facet: author code gets a real, private SQLite that cannot see
platform tables, and a mutation commits with its ledger row in the
facet's own transaction. The app has no network (`globalOutbound: null`):
a job reaches out only through `job.fetch` steps, which the supervisor
takes.

## Where each fact lives

Every change is checked against this table.

| Thing | Source of truth | Copies must be |
|---|---|---|
| File bytes, their history, `main` and `live` | code.storage git | a local folder is a disposable working copy |
| Large file bytes | R2 (`BLOBS`), by SHA-256 | git holds a pointer; a sync resolves it |
| Tree index (path, size, SHA per pinned commit) | derived from git | the cell's copy names its pinned SHA; moved by the platform's own moves, a writer's refresh after its push, or the poll backstop |
| Manifest and declared operations | `fragment.json` in git | the cell's copy at the pin; an invalid one keeps the last good and says why (`status.code.error`) |
| Members, roles, invites | the fragment's supervisor | grants and revokes transactional, each in `events`; the registry's copy of an invite waiting on an email (decision 63) is a pointer the fragment overrules |
| Cell state (supervisor tables, the operation ledger, channels, the app's SQL) | the Durable Objects' own storage | none |
| A fragment's search index | what its app writes (`…/search/upsert`), held in a `SearchIndex` object of the life's own (docs/api.md, A fragment's search) | a projection its app writes again whole when asked (a new embeddings space empties it); the supervisor holds who reads it, the forgets still to send, and whether the life wrote one (`MetaKey::Search`) |
| A platform object's schema | its class's steps, `crates/core/migrations/<class>/` (Schemas, below) | each object's `_schema`, which refuses code whose steps differ; `schema.lock`, which `cargo xtask check` holds the tree to |
| The release a call between objects was made by | the caller's `FRAGMENT_DEPLOY_ID` (Calls between releases, below) | stamped on each call and checked first by the object; never stored. Its deploy time (`FRAGMENT_DEPLOYED_AT`) is stamped too, read by no release (the debt ledger) |
| The release a platform page runs | the page as answered (its `fragment-release` meta), and each answer's `x-fragment-deploy` | the page reloads once they differ, at a safe moment, never in a loop (docs/api.md, The shell, "A new release") |
| A chat's unsent words | the chat's page | its origin's `localStorage` (`fragment.chat.draft.v1`), for the principal it was typed as alone; emptied as sent, and when its person signs out |
| Identities and keys, a person's own key (sealed), sign-ins and verified emails, agents' owners, sessions | the registry (decisions 45 to 50) | an npub never changes; an email names at most one person; no session or cache outlives a revocation; cookies hold tokens, the registry their hashes |
| Public addresses | the registry's `public_addresses`: one label, one fragment npub, never reassigned | the router resolves them there and the fragment checks the npub; a delete or wipe keeps the reservation |
| A person's wipe | the registry's `wipes` (docs/api.md, "Operators: wiping a person") | a wiped person's ledger, list and computer each keep one row saying so |
| Secrets | the object that owns each, sealed; the deployment's own in its Secrets Store (docs/secrets.md) | never in a repo, a log, a command line, or an app's env |
| A person's connections | WorkOS Pipes (one account a provider, or several: `accounts_max`); one being made, the registry's `connection_signins` (docs/pipes-browser-binding.md) | which account each agent uses, its computer's `agent_accounts`; a guest holds placeholders only |
| Orgs, their admins and seats | the registry (docs/billing.md) | each holder's plan and standing pushed to their ledger in the registry's order (`SetSeat.seq`); the registry is the seat's one writer |
| An org's credit and monthly budget (decisions 66 to 68) | its ledger (`org:<id>`) | none: operators grant and set them; its billing admins read them |
| An org's content: its people, content admins, offers and governance log (decisions 69 to 71) | the registry (`org_content`, `org_content_offers`, `org_governance`; rules in `fragment_core::governance`), apart from its billing | none: each change is written with its log entry in one transaction; an offer is taken only while its grounds hold |
| Whether a person is a guest | their seat, else their ledger's plan (`fragment_core::org::guest`) | `GET /api/seat`'s `guest`, decided at each ask |
| What is sold, at what price, and a seat's included credit | core's constants (`fragment_core::stripe`, `ledger::SEAT_INCLUDED`) | Stripe's Prices, refused at any other amount; pages read `GET /api/seat`'s `offer` |
| A subscription, and an org's Stripe customer | Stripe | the registry's `orgs`, written only from a subscription fetched from Stripe, never from an event's own copy |
| The image a computer's next start runs | each image name's reference in the Worker version; a computer's pin names one | the Computer DO's last look, recorded as the image each life runs |
| An agent's name and picture | its fragment at live: `meta.title` and `avatar.png` (docs/api.md, "An agent's name and picture") | its cell's face, and everything read from it; its computer's profile and SimpleX profile follow a rename; no chat title or `SOUL.md` copies it; its label never changes |
| An agent's role | its fragment at live: `meta.role` | its status's `agentRole`, which its runtime tells it at each turn |
| An agent's runtime, tier, or its own models and efforts | its `agent.json` in git (`runtime`, `hermes` unless named; docs/computers.md) | its computer's bridge and profile config; Settings and the agent's page read it |
| An agent's home | its `agent.json` `home` in git (docs/api.md, "An agent's home") | read by the shell, its computer's profile and the bridge; none kept |
| An agent's SimpleX chat | its owner's chat `<agent label>-simplex`, the agent in it | its name in the link's file, chosen once, before the chat is made |
| An agent's SimpleX door | the agent fragment's `meta` `door`, written only by redeeming its owner's latest ticket (docs/api.md, "An agent's doors") | its private key on the computer; a relayed record is taken only signed by the key the fragment holds |
| A hosted door: which agent's door a connector fragment is | the connector fragment's `meta` `door_actor`, set and cleared by its owner in person, and `door_template` (docs/api.md, "An agent's doors") | the agent fragment's `door.hosted` event; each publish reads the binding, the installed code and the deployment's `connectors` |
| Which connectors may be linked, and each one's relay targets | the deployment's `connectors` (`FRAGMENT_CONNECTORS`; absent, `connectors::Policy::default`) | none: every door and relay asks `Policy::authorize` at the time |
| An agent's colour | its identity, through one function (`agent-color.js`) | none: never stored |
| The model route's models, and each tier's Fast and Deep | the deployment's `models` (`FRAGMENT_MODELS`) | ids only, in each computer's view and its owner's; `GET /v1/models` reads the config |
| A computer's agents | the Computer DO's `agents` | the agent fragment's `computer` and the registry's `agent_fragments`; a delete unassigns it (cell/src/ended.rs) |
| A computer's XFTP relays | the Computer DO: leases in memory, the day's bytes in `relay_day` | none |
| A fragment's scheduled records | its supervisor's `record_schedules` (docs/api.md, "Scheduled records") | each fire appended once, keyed by schedule and minute; a computer keeps none |
| Money | each payer's ledger (docs/ledger.md) | meters batch rows to it, idempotently; each isolate's gate keeps an answer a minute at most |
| A job's paid AI answer | the supervisor's `paid` receipts, by incarnation, run and step | its charge in the meter outbox (`step:<key>`), charged once; no provider call once a receipt exists |
| Audit trail | the `events` channel | pin moves recorded as events |
| The agent docs | `cli/SKILL.md` and `cli/GUIDE.md`, naming no host | compiled into the CLI and the release's files (`/llms.txt`), each filled with its host |
| The release's files: templates, the shell's, the agent docs | the repo | the cell's index and its Static Assets, by hash, from one build; the CLI embeds the templates |
| A fragment's icon | its meta, naming one of its blobs (cell/src/icon.rs) | members' list rows carry its address |
| A person's session archive | its conversations' files, `sessions/<key>.json`, in a fragment of theirs of kind `sessions` (templates/sessions/README.md) | its indexes (`index/<kind>.json`), a projection rewritten by each import; its search, a projection of `sessions/` each import writes (a source a file, its revision the file's sha256) and reconciles against the indexes; the store an import read is never written |
| A fragment's preview card | its own storage (`fragment_core::card::Cards`) | status's `page`; errors also an `events` row |
| What members are doing: each one's newest listed record | the channel's records | the fragment's `listed`, and each member's list row (docs/api.md, Listed records) |
| A person's own view of their fragments (archived, docked, read up to) | their list, the `Principal` cell | none |
| A person's export | their list, the `Principal` cell, and its archive in R2 | the archive is a copy as read, never updated, deleted after seven days (docs/api.md, Export) |

## Schemas: the platform objects' steps

Each platform object class (`Fragment`, `Registry`, `Principal`, `Ledger`,
`Computer`) declares its schema as numbered steps, one SQL file each
under `crates/core/migrations/<class>/`, which `fragment_core::migrate`
runs as an object starts, in one transaction. An object refuses its code
(`schema_refused`, naming why) when it holds a newer step (a rollback),
another code's step of the same number, or tables it cannot adopt:
nothing is guessed, and a reset is the operator's call. A refused object
still runs what removes everything (a delete, a wipe's end), except the
Registry, which refuses everything; its recovery is Paul's call.

To change a schema, add a step: its file, its `step!` at the end of the
class's `steps` (crates/core/src/migrate.rs), then `cargo xtask
schema-lock`, and commit the three. A step is never edited, renumbered
or removed once merged. `cargo xtask check` fails a schema changed any
other way, and a statement in the cell that makes, alters or drops a
table (xtask/src/guards.rs).

No rollback to code from before the versions (#383): it ignores
`_schema`, and its wipes and deletes remake tables from step 1, dropping
later steps' columns.

## Calls between releases

A deploy does not move the Worker and its objects at one instant, so
every call into a platform object carries its caller's release
(`FRAGMENT_DEPLOY_ID`, the deploy's commit), and the object admits only
its own (`fragment_core::release`). Each call also carries when its
release was deployed (`release::DEPLOYED`), which no release reads now
(the debt ledger).

- **The caller** calls an object only through `routed::send` and
  `send_again`, the JavaScript through `InternalRoute.request` (xtask's
  guard holds both).
- **The object** checks the release before it reads the call: another
  release's, or none, is 503 `deploying`, marked
  `x-fragment-release-refused`, and nothing was done.
- **On a refusal** the caller asks again (`release::ASKS`), then answers
  `deploying` itself. An outbox, a queue message or a Workflow step
  retries it as any failure. A `deploying` an object answers because its
  own call was refused is never asked again: it may have acted. The model
  intercept marks a direct refusal as not sent to a provider
  (`release::MODEL_NOT_DISPATCHED`), so an image may wait and ask again
  (docs/computers.md, Models).
- **Every object takes a deploy's code at once** (`code_update_strategy`
  `immediate`, cell/wrangler.jsonc), not after Cloudflare's default of up
  to five minutes. A reset costs what any restart does: requests in
  flight end, sockets close, an alarm running runs again, and a
  computer's container runs on.
- **No object is restarted for a release.** A new caller at an old
  object is admitted once Cloudflare moves the object onto the new code,
  at once under `immediate`; an old caller at a new object is refused
  until its asks run out.
- **What outlives a deploy is not a call.** A queue message or a stored
  row one release writes, the next reads, so its shape stays readable.
  A release tolerates what the next writes where it can: a run's `via`
  it does not know reads as written, and a trigger kind it does not know
  starts nothing. A value an older release cannot read makes its release
  forward only, by a schema step (Fragment step 6, FIN-328).

A deploy is all at once (a gradual deployment would refuse between its
two versions), and done when every object class answers it within 90 s
(`/healthz/objects`; `VERIFY_LIMIT`, xtask/src/rollout.rs): an object
Cloudflare holds on its old code fails the deploy loudly, rather than
refusing calls that long. One it cannot finish or verify goes back to
the version before (`wrangler rollback`, verified the same way), unless
that release's schema (its steps, and its last Durable Object migration)
is not its own; the job fails either way, never
green over a mix. A platform page outlives a
deploy too, and reloads once the platform's release differs from its own
(docs/api.md, The shell, "A new release").

## Limits

Every limit is a named constant with its reason beside it, in
`fragment_proto::limits` (crates/proto/src/lib.rs) unless it belongs to
one module; each is enforced and tested. A value lives there, never in a
doc. Channels keep what their constants say (`AUDIT_RETENTION_MS`,
`AUDIT_KEPT`, `POSTED_KEPT`); an app's other channels keep everything.
