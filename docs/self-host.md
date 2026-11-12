# fragment self-hosted

Self-hosting runs master's cell on other implementations of the interfaces
it is written against. It lands on master seam by seam: each is switched
on by configuration, and master's Cloudflare path is unchanged. Where this
disagrees with `docs/cloudflare-v1.md`, that doc wins.

## The rule: one cell, interfaces, placement as configuration

The cell is written against Cloudflare's runtime APIs (Durable Objects,
the Worker Loader and facets, Workflows, Queues, R2, `ctx.container`) and
protocols for what sits outside a runtime (OpenAI's chat completions, the
code-store contract, the DevTools Protocol, Web Push). Self-hosting
supplies other implementations of the same interfaces. The cell never
asks "am I self-hosted?": it reads what it has from its configuration and
its bindings. A mode flag is a design bug.

## The seams

| # | Seam | On master |
|---|---|---|
| 1 | The runtime: celld | dev and the e2e (below) |
| 2 | Computers on sandcastle nodes | not yet: no computers on celld |
| 3 | Models from a file | dev's `FRAGMENT_MODELS_FILE` |
| 4, 9 | Sign-in by OpenID Connect; LAN mode | not yet; locally, dev's WorkOS fake |
| 5 | Files: macrofiche, the code-store contract | not yet: the code.storage fake |
| 6 | Blobs | nothing to port: celld's R2, over `celld dev`'s disk |
| 7 | Preview cards: a renderer | not yet: a card is skipped, saying why |
| 10 | Money: own hardware in points | not yet |
| 12 | Secrets | the stand-in (below) |
| — | Mail | debt: said and carried on |

## 1. The runtime: celld

celld (github.com/futurepaul/celld, its `selfhost` line: upstream's
v0.6.3, still its latest, and the fork's parity fixes, among them V8
15.2.124.45, far alarms, a forwarded body's length and facet aborts) runs
the cell from a config rendered for it, `wrangler.celld.jsonc` beside the
cell's own (`crates/devstack/src/celld.rs`, `DROPPED`), less what celld
has no part in. One `devstack::Node` runs either runtime behind the
node's front door (front.rs): celld listens on a port of its own, which
the front door is told, as it learns workerd's from wrangler's. Stop
(SIGINT), crash (the group SIGKILLed) and the e2e's signal handler serve
both.

Every binding and runtime API master's cell uses, against the fork's
`selfhost` line:

| Binding or API | On celld | Class | How the cell runs |
|---|---|---|---|
| `durable_objects`: Fragment, Principal, Registry, Ledger, SearchIndex, Vault | yes | works | |
| `durable_objects`: Computer | the class runs | off | no computers: `FRAGMENT_COMPUTER_IMAGE` unset, so a computer is refused, typed ("this deployment makes no computers"); its container host reads no container as none running, so a person's wipe still empties their computer's rows |
| `durable_objects`: Migrations, and R2 `STAGE` | yes | off | the finite.computer move: unconfigured (`FRAGMENT_MIGRATIONS_MOVER` unset), as on any deployment but finite.chat's |
| `migrations` (`new_sqlite_classes`, v1 to v4) | yes | works | |
| `durable_objects.code_update_strategy` | accepted, no effect | works | celld reads the bindings alone; `celld dev` deploys once (`--no-watch`), and the e2e's reload restarts the node |
| `worker_loaders` (`LOADER`) and facets (`ctx.facets`) | yes | works | apps' facets, their databases capped past the cell's own (`CELLD_FACET_MAX_BYTES`), loaded code locked down (`CELLD_DYNAMIC_LOCKDOWN`); `facets.abort` releases a brain's facet (found 51, fixed in the fork) |
| `workflows` (`JOBS`) | yes, durable | works | a sleep is an alarm, so a job sleeping through a crash wakes (local workerd's does not) |
| `queues` (`DELIVERIES`, `METERS`, the dead letters) | yes | works | |
| An object's requests in flight | 64 at most by default | works, configured | devstack runs celld with 512 (found 50) |
| `r2_buckets` (`BLOBS`) | yes; its class reads as `Object` | works | read by what it does (`js::by_what_it_does`, found 35) |
| `assets` (`ASSETS`, `run_worker_first`, `html_handling`, `not_found_handling`) | yes; its class reads as `Object` | works | read by what it does (found 35) |
| `secrets_store_secrets` | refused | stood in | the cell's stand-ins (seam 12); people's secrets stay in the Vault object |
| `ai` (`AI`: Workers AI) | refused | degraded, typed refusal | text tiers on an OpenAI-compatible API (seam 3); a slot left on Workers AI, image steps, transcription and embeddings fail typed ("this node has no AI binding"). Dev and the e2e keep the fake (`FRAGMENT_AI_URL`) |
| `send_email` (`EMAIL`) | refused | degraded, said | no mail: each send logs `mail.unsent` and answers no id, never an error |
| `browser` (`BROWSER`) | refused | degraded, said | no cards: a live is skipped once, `card.skipped` `no_browser`; asking for a shot again is refused, typed (`host_failed`). Seam 7 brings a renderer |
| `images` (`IMAGES`) | refused | degraded, typed refusal | no icons drawn or fitted: refused at once, typed (`host_failed`), before anything is charged |
| `containers` | runs on Docker, but without `interceptOutboundHttp`/`interceptOutboundHttps` or `snapshotContainer` (krun engine only) | off | a computer needs its egress intercepts and its saves: dropped by the render; seam 2 places computers on sandcastle nodes |
| `ctx.exports` | yes (`DurableObjectState.exports`) | works | used only by a computer's container host |
| `interceptOutboundHttp`, `interceptOutboundHttps`, `snapshotContainer` | krun engine only | off | with computers (seam 2) |
| `connect()` (`cloudflare:sockets`) | yes | off | the XFTP relay is a computer's |
| `compatibility_flags`: `nodejs_compat` | partial (its `node:` modules) | works | the Sandbox SDK's imports load |
| `compatibility_flags`: `containers_pid_namespace` | accepted, no effect | off | with computers |
| `build` | refused | n/a | worker-build has run (`cargo xtask build`) |
| A streamed request body forwarded to an object | its length carried, as workerd's (found 47, fixed in the fork) | works | |
| `CF-Connecting-IP` | not set | degraded | every CLI code's ask counts as one address, `unnamed`, unless a front proxy sets the header (found 48) |
| Cache API, HTMLRewriter, cron triggers | | n/a | the cell uses none |

- **Dev:** `cargo xtask dev --runtime celld` (`CELLD_BIN`; esbuild from
  worker-build's cache or `CELLD_ESBUILD`): the same fakes and settings as
  on wrangler, no computers, the secrets behind the stand-ins.
- **The e2e:** `FRAGMENT_E2E_RUNTIME=celld`. Its rung (`needs::Rung::Celld`)
  has everything a local run has but the runtime's containers and the
  finite.computer move: a section that needs computers is a skip saying
  seam 2. Checks celld cannot make are skips saying why (a card's shot,
  an icon's making; found 31's 413); the check local workerd cannot make
  runs (a job sleeping through a crash). The build makes no computer
  images.
- **Master's path is unchanged:** nothing here branches on the runtime.

## 12. Secrets: the store's bindings, stood in for

The cell reads each deployment secret as `await env.<BINDING>.get()`
(cell/src/keys.rs, through workers-rs's `SecretStore`). A self-hosted
runtime has no Secrets Store (celld's deploy refuses
`secrets_store_secrets`), so `cell/secrets.mjs` gives the cell stand-ins
with the same face: every class in `cell/entry.mjs` that runs the
platform's code takes its env through `withSecrets`, which, when the
Worker variable `FRAGMENT_SECRETS` names the deployment's secrets (the
pairs a deploy writes into `secrets_store_secrets`, and a backend), puts
an object with an async `get()` in each binding's place. Its class is
named `Fetcher`, as workerd's is, since workers-rs takes a store binding
by that name. Without `FRAGMENT_SECRETS` (Cloudflare, `wrangler dev`) the
env is as it came; the Rust is the same on both and never knows which it
read.

- **`vars`:** the Worker variable of the binding's own name, in
  `.dev.vars` (`devstack::Secrets::Shim`): a convenience for dev and the
  e2e alone. The shim refuses it on any fleet that is not local
  (`FRAGMENT_EGRESS_LOCAL=allow`).
- A `FRAGMENT_SECRETS` that names no backend is refused at the first
  request, loudly; nothing falls back to a backend it did not name.
- People's own secrets are not here: they stay in master's Vault Durable
  Object, sealed under keys derived from the host secret, as on
  Cloudflare.
- The e2e checks the stand-ins on wrangler: `FRAGMENT_E2E_SECRETS=vars`
  runs its node with them (its local config binds no store).

## What celld does differently, still true

The old lane's numbering is kept.

8. **`celld dev` stops on SIGINT**, not promptly on SIGTERM: devstack stops
   it so.
21. **celld ties a WebSocket to the request that opened it**
    (denoland/celld#254): seam 2's.
22. **celld bounds an outbound fetch at 120 s** (`CELLD_FETCH_TIMEOUT_S`):
    a model call slower than that is cut.
31. **celld's early 413 to an upload it refused can be lost** (hyper
    closes on the unread body): the e2e skips reading that 413 on celld,
    and still checks nothing was charged or sent.
34. **`celld dev` reads its whole bucket for every listing:** the fork
    carries a range query as a stopgap; upstream's #265.
35. **celld's R2 and assets bindings' classes read as `Object`,** so
    workers-rs's typed reads refuse them: the cell reads `BLOBS`, `STAGE`
    and `ASSETS` by what they do.
45. **`celld dev` fences itself when one lease renewal stalls** past its
    10 s TTL (`SELF-FENCE: node lease not renewed within TTL`, exit 3): its
    store's SQLite waits on a disk busy writing back. Off that disk it does
    not stall: an e2e run, whose state is thrown away, can keep it on
    tmpfs.
48. **No `CF-Connecting-IP` on celld:** the cell counts a CLI code's asks
    by it, so every address is `unnamed` and shares one bucket. A front
    proxy that sets the header (a tailnet's, a company's) gives each its
    own.
49. **The dev WorkOS fake's adoption reads wrangler's state layout**
    (`devstack::signed_in_subjects`): on celld it finds no one, so a dev
    stack on celld keeps its fake's subjects in the fake's own file alone.
    A celld state that outlived that file is refused at start
    (`celld::workos_users_kept`; `--clean` starts afresh): a fresh fake
    would hand a known subject to another email.
50. **celld caps an object's requests in flight at 64**
    (`CELLD_MAX_CELL_REQUESTS`; past it, 503 "cell request limit
    reached"). Cloudflare queues an object's requests with no such count,
    and one fragment's 120 triggered runs an hour pass it. devstack runs
    celld with 512, at least twice the triggered runs (a const assertion
    holds it).
53. **celld runs each object in an isolate of its own,** and the Worker
    over a pool of them, where local workerd runs the Worker and every
    object in one. What one isolate's memory holds (the ledger gate, which
    the levers forget and read) is then another isolate's, as on
    Cloudflare. On celld the e2e keeps what callers see (the next call
    refused, 402) and skips what a lever reads of the gate
    (`Suite::one_isolate`).

Fixed since, in the fork or here:

- 47, a streamed body forwarded to an object lost its length: the fork
  keeps it, as workerd does, so the cell forwards nothing of its own.
- 51, `facets.abort` failed for a brain's facet ("adopt failed"): the fork
  releases a facet that locks its own `facets`.
- 52, an alarm years ahead panicked the node at every start: the fork
  arms a far deadline at its timer queue's bound. The move stays off on
  self-host all the same (`Need::Move`).
- 54, a slides presenter's Stop "lost": an e2e race, not celld's (the
  check read the channel before the record's post landed).

## What a self-hosted org won't have, and its setup steps

**A working list** (2026-11-11), for one box on a tailnet with outbound
internet, no inbound internet, and the cell on celld. Revisit after the tailnet design: it would remove step 4 (the
org's own WorkOS: the tailnet's identity instead), and change steps 1 and
2 (MagicDNS names and a box CA pinned in the apps, the org's own domain
optional) and the iOS row (the app's own trust and tailnet).

### Works unchanged, given outbound internet

| Feature | Depends on | Note |
|---|---|---|
| Web push alerts | each fragment's VAPID key; the browsers' push services | Queues work on celld |
| Claude runtime | the person's own Claude plan | its code-paste sign-in needs nothing inbound |
| Own OpenRouter key (`own` provider, PKCE) | a browser redirect to `/api/connections/openrouter/callback` | the tailnet is enough |
| SimpleX door | simplex-chat in the computer; SMP and XFTP on 443 | needs an always-on computer (seam 2) |
| code.storage sync | polling | the account itself needs setup (below) |
| Jobs' `fetch` to public hosts | outbound | |
| CLI approval at `/cli` | nothing | its "asked from city/country" line is blank (no `request.cf`) |
| CLI update notice | speaks only when its host is finite.chat | silent elsewhere |
| In-app invites, sharing to named people, vanity addresses | the registry | |
| Export, wipe, keyword search, Vault, doors | the platform | a wipe needs no container (`ContainerHost` reads none running) |
| Web shell fonts | fonts.googleapis.com, fetched by the client | offline clients fall back |

### Works with extra setup

| Feature | Finite's today | What the org brings |
|---|---|---|
| Sign-in | Finite's WorkOS AuthKit | its own WorkOS environment: redirect `https://<platform>/auth/callback`, invite URL `/auth/login`, logout `/`; `workos.client_id`, `workos.api_key` |
| Fragment files (git) | Finite's code.storage org | its own code.storage org and PEM key (`codestorage.*`); its files then live at that vendor, off the box, so macrofiche (seam 5) matters |
| Text models | OpenRouter on Finite's key, or Finite Private | an OpenAI-compatible endpoint and key for every slot (`cheap`, `medium`, `fallback`, `vision`, Deep), each priced; a slot left out falls back to Workers AI, which celld lacks. A deployment's `base_url` is https on a host name, no port; dev's models file takes this machine's `http://127.0.0.1:<port>` (seam 3) |
| Semantic search (embeddings) | Workers AI bge-m3 | an OpenAI-compatible `/embeddings` endpoint, such as TEI (`models.embeddings`) |
| Agents' paid tools (X, Firecrawl, fal, Browser Use, Perplexity, Places, xAI, ElevenLabs) | Finite's operator keys | its own key per row, or the row dropped |
| Google, Linear and Salesforce connections | Finite's WorkOS Pipes and OAuth clients | its own OAuth client per provider in its own Pipes; redirect `https://<platform>/api/connections/<provider>/callback`; for Google, an internal Workspace app |
| Usage and credit | every call charged; guests and zero balances refused | no Stripe; `default_plan` `seat` or `seat_always_on`; `operators` grant credit; own keys still priced by the book |
| Apps calling internal or tailnet services | public addresses only | blocked: 10/8, 192.168/16, 100.64/10 (the tailnet's), `.internal`, single-label names; only `FRAGMENT_EGRESS_LOCAL=allow` lifts it, which is never for a shared fleet. **A decision for an intranet** |
| CLI | defaults to finite.chat | `fragment login --host https://<platform>`, or `FRAGMENT_HOST` |
| Hermes' platform skill text | names finite.chat at image build | the image built with `FRAGMENT_HOST` set (not verified) |
| iOS app | defaults to finite.chat; Release builds take https only | the org's server address typed in; today that needs a publicly trusted certificate, or a CA profile on every phone |
| Legal pages and contact | Finite Computer LLC's About, Privacy and Terms | not configurable: edited, or lived with; iOS links finite.chat's |
| Upload size, support link | a zone's 100 MB | `upload_max_bytes`, `support_url` |

### Won't work without public inbound reachability

| Feature | Inbound need | Alternative |
|---|---|---|
| A fragment's inbox (apps' webhooks, the `inbox` template) | `POST /api/f/<name>/inbox` | poll from a cron trigger, or expose that path alone through a tunnel (not checked) |
| Telegram connector | its webhook into the chat's inbox | **off on self-host:** a self-hosted stack's connectors are SimpleX alone (`FRAGMENT_CONNECTORS`; devstack's `celld::connectors`). `getUpdates` long-polling from the computer is not built. The shell offers Telegram by an agent's runtime, not by the deployment's policy, so once computers come to self-host, hiding its row is a wire change (today no agent runs on celld) |
| Public and "anyone with the link" sharing | visitors outside the tailnet | "public" means the tailnet |
| Stripe | `/api/stripe/webhook` | left out |

OAuth redirects (WorkOS, Google, Linear, Salesforce, OpenRouter) go
through the person's browser, so the tailnet is enough.

### Not on self-host at all

| Feature | Why | On celld today |
|---|---|---|
| Voice transcription (iOS voice memos too) | Whisper on Workers AI | a typed refusal (no `AI` binding); a later seam moves it to OpenRouter |
| Job steps `ai.image`, `ai.video`, `ai.decide`; embeddings | Workers AI (video through AI Gateway too) | a typed refusal; a later seam |
| Drawn icons, and fitting uploaded ones | Workers AI and Cloudflare Images | refused at once, typed (`host_failed`, "no Images binding"); a 256×256 PNG is kept as it is |
| Preview cards | Browser Rendering | skipped, said once (`card.skipped`, `no_browser`); seam 7's renderer |
| Platform email | Email Sending | `mail.unsent`, never an error (debt) |
| Buying seats and credit | Stripe, its inbound webhook | operators comp seats and grant credit |
| Operators' glance, its logs half | Workers Observability | the registry's numbers still show |
| The finite.computer move | the Migrations object, `STAGE`, `/move` | off: unconfigured, hidden |
| Finite Private | a Tinfoil key | |
| Phone push (APNs) | not built anywhere | missing on finite.chat too |
| Deploy tooling | wrangler, a Cloudflare zone | `celld deploy` has no xtask path yet; this phase is the dev stack |

### Setup steps for the org's IT person, in order (master as it is today)

1. Domain and DNS: a platform host and a wildcard fragment suffix pointed
   at the box's tailnet address (public DNS, or the tailnet's split DNS).
2. TLS: a publicly trusted certificate and a wildcard (ACME DNS-01).
3. The front door sets `CF-Connecting-IP` (found 48): without it every
   CLI code's ask counts as one address, and a client could name its own.
4. Secrets: behind the cell's stand-ins (seam 12); the host secret, 32
   or more random bytes, among the platform's root secrets there.
5. WorkOS: its own environment and the redirects.
6. code.storage: its own org and key (or macrofiche, once ported).
7. Models: endpoints for every slot, and embeddings.
8. Plans and operators: `fragment operator key`; `operators`;
   `default_plan`.
9. Computers: the Hermes image built (with `FRAGMENT_HOST`) and loaded
   into the engine (seam 2).
10. Providers: rows dropped or keys brought; OAuth clients for Google,
    Linear and Salesforce.
11. Connectors: SimpleX alone, for now.
12. Left out: `stripe`, `mail_from`, `ai_gateway`, `observability_read`,
    `migrations`, `test_secret_file`.
13. Devices: the tailnet joined; the CLI's `--host`; the iOS server field.
14. Legal pages.
15. Tell people what is missing: voice memos, AI images, video and
    decisions, icons and cards, email, webhooks and Telegram,
    tailnet-only "public", and no internal services.

**Least certain:** a tunnel for the inbox, OpenRouter's PKCE callbacks
behind a tailnet name, and the Hermes `FRAGMENT_HOST` rebuild.

## Running it

```sh
# celld: the fork's selfhost line (cargo build --release -p celld)
CELLD_BIN=…/celld cargo xtask dev --runtime celld
# the e2e on celld
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… cargo xtask e2e --only auth,create,…
```

## Not yet

- Computers (seam 2): none on celld. celld's Docker engine has no egress
  intercepts or snapshots, which a computer needs; its krun engine
  (sandcastle's) has both.
- Sign-in by OpenID Connect (seams 4, 9).
- Own hardware in points (10), cards (7), macrofiche (5).
- `celld deploy`: no xtask path yet; this is the dev stack and the e2e.
