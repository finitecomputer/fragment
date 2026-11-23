# Security material for Google's CASA assessment

This folder answers the security assessment (CASA) that Google asks
for as part of verifying Finite's restricted Gmail and Drive scopes. Our
understanding, to confirm with Google and TAC Security: an unverified
app is capped at 100 users, verification of restricted scopes needs a
CASA letter, and TAC Security assesses against OWASP ASVS. Each page states what the code and the
deployment do today, with file references, and lists what is not done.
Policy pages are drafts until Paul approves them.

Security owner for the assessment: Alex Lewin (Linear FIN-570).

## What Finite is

Finite (finite.chat) is a personal-agent product run by Finite Computer
LLC. A person signs in, gets a computer (a container of their own) and
one or more agents, and chats with them. Agents build and share small
multiplayer web apps called fragments. The whole platform runs on
Cloudflare (docs/cloudflare-v1.md, decisions 1 to 5). The code is this
repository: one Rust Worker (`cell/`), its Durable Objects, and the
images computers run (`images/`).

A person may connect accounts such as Google. Their agents then call
Google's APIs for them, from their computer. The platform never gives
the computer the Google token: the agent holds a placeholder, and the
platform swaps the token in at the computer's egress (docs/computers.md,
"Connections and operator keys"). prompt-injection.md says where that
stops short: Google's answers come back unredacted.

## Pages

| Page | Covers | Linear | State |
|---|---|---|---|
| [README.md](README.md) | what Finite is, scope map, Google data flow | FIN-572 | this PR |
| [logging.md](logging.md) | what is logged, where, for how long; what is kept out | FIN-586 | this PR |
| [prompt-injection.md](prompt-injection.md) | how an agent reading untrusted content is constrained | FIN-589 | this PR |
| [secrets-and-encryption.md](secrets-and-encryption.md) | secret custody, encryption in transit and at rest, rotation | FIN-590 | this PR |
| [incident-response.md](incident-response.md) | detecting, containing and telling people about an incident | FIN-591 | draft, needs Paul |
| [vulnerability-management.md](vulnerability-management.md) | finding and fixing vulnerabilities, by severity | FIN-592 | draft, needs Paul |
| [production-access.md](production-access.md) | who can reach production and user data | FIN-593 | draft, needs Paul |
| [secure-development.md](secure-development.md) | how code ships: PR, proof, review, CI, preview, production | FIN-594 | this PR |
| http-headers.md | security headers and HSTS | FIN-582, FIN-575 | in progress (another PR) |
| dependency-scanning.md | scanning dependencies for known vulnerabilities | FIN-576 | in progress (another PR) |
| google-oauth.md | the Google OAuth flow | FIN-580 | in progress (another PR) |
| sessions.md | session handling | FIN-581 | in progress (another PR) |
| access-control.md | people can't reach each other's data | FIN-584 | in progress (another PR) |
| uploads.md | untrusted uploaded files | FIN-583 | in progress (another PR) |

Background contracts, already in the repo: docs/MODEL.md (where each
fact lives), docs/secrets.md (secret custody), docs/computers.md
(computers and their egress), docs/pipes-browser-binding.md (connecting
an account), docs/api.md (the wire, operators, export, wipe).

## Scope map (FIN-572)

### Hosts

| Host | What it is | Source |
|---|---|---|
| `finite.chat` | the platform host: the shell (sign-in, chats, settings), every `/api/…` route, `/auth/…`, the OAuth callbacks, and the operators' `/admin` | docs/cloudflare-v1.md decision 5; docs/api.md |
| `<name>.finite.place` | each fragment (an app, a chat, an agent's page, a site) on its own browser origin, on a site apart from the platform's | decision 5; docs/MODEL.md ("Each fragment is its own browser origin"); R16 |
| `<24 hex>--computer.finite.place` | a computer's own origin: its ports (`/p/<port>/…`), such as its screen, to its owner only | docs/computers.md, "Ports"; decision 41 |
| preview | one shared preview, deployed by master's green CI run as the branch deployment `preview` at `preview.<zone>`, its fragments at `<name>--preview.<zone>`. Another branch deploys only when someone runs `cargo xtask deploy --branch <name>` by hand | decision 20; `.github/workflows/ci.yml` (`deploy`) |

The exact preview zone and any other hostnames are in the private
deployment configs (finitecomputer/fragment-deploy) and are not copied
here.

### Components

| Component | Where it runs | Holds | Source |
|---|---|---|---|
| The cell | Cloudflare Worker (Rust, workers-rs) | nothing of its own; routes every request | `cell/src/lib.rs`, decision 2 |
| The admin Worker | a second Worker from the same crate | the operators' `/admin` and `/api/admin/…` | `cell/src/admin.rs`; docs/api.md, "Operators: the admin" |
| Registry | Durable Object | identities, sign-ins and verified emails, session hashes, a person's sealed key, orgs and seats, connection sign-ins under way, the operators' log | docs/MODEL.md; `cell/src/registry.rs` |
| Fragment (supervisor) | Durable Object, one per fragment | members, invites, channels (chats, the `events` audit trail), schedules, sealed secrets | docs/MODEL.md table |
| App | Durable Object Facet per fragment | the app author's SQLite; no network (`globalOutbound: null`) | docs/MODEL.md |
| Principal | Durable Object, one per person | their list of fragments, their search index, their export | docs/MODEL.md |
| Vault | Durable Object, one per person | their sealed secrets for their agents and their own keys | docs/secrets.md, "A person's Vault"; `cell/src/vault.rs` |
| Ledger | Durable Object, one per payer | usage and credit | docs/ledger.md |
| Computer | Durable Object, one per computer | the container's lifecycle, its saves, its egress intercepts | docs/computers.md; `cell/src/computer.rs` |
| SearchIndex | Durable Object | a fragment's search index | docs/MODEL.md |
| Migrations | Durable Object | the move of old finite.computer agents | docs/migrations.md |
| Containers | Cloudflare Containers, one per person | the agents' runtime (Hermes), the bridge, the agent's working files in `/data` | docs/computers.md; `images/` |
| R2 `BLOBS` | Cloudflare R2 | large file bytes by SHA-256; exports | `cell/wrangler.jsonc`; docs/MODEL.md |
| R2 (container saves) | Cloudflare R2, through the sandbox SDK's `DirectoryBackup` | each computer's `/data` saves | docs/computers.md, "Saves and what a wake restores" |
| Workflows, Queues | Cloudflare | job runs; deliveries and web push | docs/MODEL.md table |
| Secrets Store | Cloudflare | the deployment's own secrets | docs/secrets.md |
| AI Gateway, Workers AI | Cloudflare | model calls. Each Workers AI call through the gateway sets `collectLog: false` (`cell/src/models.rs`), and a deploy refuses the `default` gateway (`xtask/src/deploy.rs`). Retention at Workers AI and the gateway's live settings: to confirm | decision 23 |

### Processors (outside services)

The public privacy policy (https://finite.chat/privacy, last updated
2026-10-22) names these. The column on the right says how the code uses
each.

| Service | Its part | In the code |
|---|---|---|
| Cloudflare | hosting, storage, computers, some AI models, email | all of the above |
| code.storage | git for every fragment's files and each agent's self | docs/MODEL.md ("Files"); decision 1 |
| WorkOS | sign-in (AuthKit) and connected accounts (Pipes, which holds and refreshes their tokens) | decisions 1, 22; docs/pipes-browser-binding.md |
| Stripe | payments; Stripe holds card details | docs/billing.md |
| OpenRouter | model calls the platform routes, and a person's own OpenRouter account if they connect one | decisions 60, 61; docs/computers.md, "Models", "An agent's own model" |
| Tool services: X, Firecrawl, fal, Browser Use, Perplexity, xAI, ElevenLabs | each gets a request only when an agent uses that tool, through an operator key swapped in at the computer's egress | docs/computers.md, "Connections and operator keys"; `deploy/example.jsonc` (`providers`) |
| Google | only for a person who connects Google | below |
| The browser's push service | only for a person who turns on notifications; the message is encrypted for their device | `cell/src/push.rs` |

The code can also send model calls to these, depending on the
deployment's config and the agent's runtime. Which ones production's
effective config uses is to confirm:

- **Anthropic**, for an agent on the Claude Code runtime: Claude Code
  runs on the computer, signed in with the person's own Anthropic
  account, and calls Anthropic itself (docs/computers.md, "Claude Code
  in our Hermes image"; `images/bridge/src/runtime/claude.rs`).
- **The embeddings provider**, for searchable content: Workers AI by
  default, or an OpenAI-compatible API the config names
  (`cell/src/searchable.rs`, `cell/src/models.rs`, `embedded`).
- **Any OpenAI-compatible API a model slot names**, such as Tinfoil
  (Finite Private) in `deploy/example.jsonc`'s commented examples.

Agents can also visit websites and use other services a person asks
for. Computers reach the internet (decision 43); those sites see what
the agent sends.

## Where Google data flows and rests

Google is a `connection` row of the deployment's catalog
(`deploy/example.jsonc`, `providers`, `google`), with hosts
`gmail.googleapis.com`, `www.googleapis.com`, `people.googleapis.com`,
`sheets.googleapis.com`, `docs.googleapis.com` and
`slides.googleapis.com`.

### 1. Connecting

1. The person, signed in on finite.chat, picks read only or read and
   write in Settings and starts the connection
   (`POST /api/connections/google/authorize` with `{access}`). Finite
   asks Google for that tier's scopes alone. The registry keeps the
   state's hash and the sealed PKCE verifier for ten minutes, bound to
   that browser by an HttpOnly cookie.
2. Google's consent screen, where the person may untick scopes.
3. Google redirects to finite.chat's callback. The Registry checks the
   cookie, the session and the attempt, then exchanges the code at
   `oauth2.googleapis.com` with the confidential client.
4. The Registry receives and parses Google's token response, and
   submits the tokens to WorkOS Pipes' import with the scopes Google
   says it granted (`cell/src/registry/connection_signin.rs`,
   `finish_taken`). The tokens are not written to any Registry row and
   are never sent to the browser (docs/pipes-browser-binding.md,
   "Tokens").

The scopes Google returns can include scopes granted earlier
(`include_granted_scopes`). So a reconnect that picks read only does not
remove write scopes granted before; only a disconnect, then a connect as
read only, does (docs/pipes-browser-binding.md, "Access"). Settings shows
the access the grant holds, not the access last asked for
(`connection_signin::access_of`).

### 2. Using

1. The agent's runtime, on the person's computer, holds only a
   placeholder (`fcx_google_<tag>`) in `GOOGLE_OAUTH_ACCESS_TOKEN`.
2. It calls a Google API host. The egress intercepts cover a selected
   set of hosts: the catalog's provider hosts and the hosts of the
   person's Vault secrets (`cell/intercepts.mjs`). For a request to one
   of them, the Computer Durable Object's swap checks that the
   placeholder's tag names an agent of that computer that may use that
   provider, and that the host is one of the provider's own. It then
   asks Pipes for an access token for this request, swaps it in and
   sends the request on over HTTPS. A placeholder for another
   provider's host is refused there (docs/computers.md, "The
   intercept").
3. A request to a host no intercept covers leaves the computer with the
   placeholder unchanged. It carries no real token, but nothing refuses
   it.
4. The tag says which agent of the computer the placeholder is for, not
   which process on the computer sent it: any process in the container
   can read and use the agents' placeholders.
5. Google's answer goes back to the agent on its computer, unredacted.

### 3. Where it can then rest

Google content is not stored by the platform on its own. It rests only
where the agent puts it while doing what the person asked:

| Place | What | Source |
|---|---|---|
| The computer's memory and `/data` | the runtime's context, transcripts and working files | docs/computers.md, "Durability" |
| The computer's saves in R2 | the newest three saves of `/data`, plus a fourth when the running computer came up from an older one, and saves set aside for 30 days | docs/computers.md, "Saves and what a wake restores"; `crates/core/src/computer/saves.rs` (`saved`) |
| A model provider, in transit | the agent's prompt, when it carries Google content: Workers AI through AI Gateway (`collectLog: false` on each call), OpenRouter (the example config asks for zero-data-retention endpoints on its OpenRouter slots; production's config and OpenRouter's retention to confirm), the person's own OpenRouter account, Anthropic on the Claude Code runtime, the embeddings provider for searchable content, or another API a model slot names (list above) | decision 23; docs/computers.md, "Models"; `deploy/example.jsonc` |
| Chat records | what the agent writes in a chat, in the chat fragment's Durable Object | docs/chat-records.md |
| The agent's repo | memories the agent writes, in code.storage git | docs/MODEL.md ("An agent's self") |
| Any fragment the agent writes to | an app's SQLite, git files, R2 blobs, search index | docs/MODEL.md |
| The person's export | a copy of what the export reads (their own fragments and their agents', their computer's newest save and newest set-aside save; a fragment shared with them only by name), in R2. A ready export expires after seven days; its objects are then deleted by a later alarm, retried until it succeeds | docs/api.md, Export; `cell/src/export.rs` (`export_alarm`, `clean`) |

The OAuth tokens are stored only at WorkOS Pipes. The Registry holds
them in memory while it imports them, and the computer's swap holds an
access token only for the request it goes on.

### 4. Disconnecting

The person disconnects from Settings, signed in; the route acts only on
that person's own accounts (`cell/src/connections.rs`, `route`). There
is no operator route that disconnects someone else's account.

A disconnect tries to revoke the grant at Google first (RFC 7009), with
an access token Pipes mints for it. The revoke is best effort: if Pipes
mints no token, or Google's revoke fails, the failure is logged and the
disconnect goes on. It then deletes the account at Pipes, which ends
every later swap of it, and answers whether Google confirmed the revoke
(`revoked`, which can be false). Last, it tells the person's computer,
whose swaps then refuse the provider at once; if that call fails, the
disconnect answers an error and the person is asked to disconnect again
(`disconnect`, `revoke_grant`, `disconnected_told`). Deleting Google
data an agent already saved is FIN-588.

## To confirm

- Alex: whether the lab's scope includes preview deployments, or
  production alone.
- Alex: whether the old finite.computer service, still used for the
  migration (docs/migrations.md) and the support address, is in scope.
- Alex: whether any API host other than `finite.chat` serves the
  platform (the FIN-572 ticket names "the API").
- Paul: the privacy policy says Finite uses one cookie; the platform
  sets several (session, share view, connection sign-in; docs/api.md).
  The wording may need an update before the lab reads it.
