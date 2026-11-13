# fragment self-hosted

Self-hosting runs master's cell on other implementations of the interfaces
it is written against. It lands on master seam by seam: each is switched
on by configuration, and master's Cloudflare path is unchanged. Where this
disagrees with `docs/cloudflare-v1.md`, that doc wins.

A pod (`docs/pod.md`) is one owner's self-hosted stack inside a
confidential VM, plus a measured image, the owner's unlock, and
attestation.

## The rule: one cell, interfaces, placement as configuration

The cell is written against Cloudflare's runtime APIs (Durable Objects,
the Worker Loader and facets, Workflows, Queues, R2, `ctx.container`) and
protocols for what sits outside a runtime (OpenAI's chat completions, the
code-store contract, the DevTools Protocol, Web Push). Self-hosting
supplies other implementations of the same interfaces. The cell never
asks "am I self-hosted?": it reads what it has from its configuration and
its bindings. A mode flag is a design bug.

**The runtime looks like Cloudflare** (Paul, 2026-11-12): celld presents
Cloudflare's binding surface, so the production cell carries no code of a
self-hosted runtime's. Each binding has workerd's class (`R2Bucket`,
`Fetcher`, `SendEmail`, …, as workers-rs checks them); the Secrets Store's
secrets are read from the node's secrets backend; and the services a node
does not run are bound with workerd's classes, every call rejecting,
saying so. A gap is closed in celld (github.com/futurepaul/celld), never
in the cell: a self-host change to master does not touch `cell/`,
`crates/core` or `crates/proto`.

## The seams

| # | Seam | On master |
|---|---|---|
| 1 | The runtime: celld | dev and the e2e (below) |
| 2 | Computers on sandcastle nodes | not yet: no computers on celld |
| 3 | Models from a file | dev's `FRAGMENT_MODELS_FILE` |
| 4, 9 | Sign-in by OpenID Connect; LAN mode | not yet; locally, dev's WorkOS fake |
| 5 | Files: macrofiche, the code-store contract | not yet: the code.storage fake |
| 6 | Blobs | nothing to port: celld's R2, over `celld dev`'s disk |
| 7 | Preview cards: a renderer | not yet: a card's shot fails (celld's `BROWSER` rejects) |
| 10 | Money: own hardware in points | not yet |
| 12 | Secrets | celld's node backend: OpenBao 2.7.2 (the default), a file of their values for `celld dev` alone |
| — | Mail | debt: celld's `EMAIL` rejects every send (`E_NO_MAILER`), a failed send |

## 1. The runtime: celld

celld (github.com/futurepaul/celld, its `selfhost` line: upstream's
v0.6.3, still its latest, and the fork's parity fixes, among them V8
15.2.124.45, far alarms, a forwarded body's length, facet aborts and
Cloudflare's binding surface) runs the cell from the config a local
wrangler node runs (`wrangler.local.jsonc`, its secrets bound by name),
less `build` (`crates/devstack/src/celld.rs`, `DROPPED`), written beside
the cell's own as `wrangler.celld.jsonc`. Its node's settings are celld's
own: its secrets backend (`CELLD_SECRETS_FILE`, seam 12) and no container
engine (`CELLD_CONTAINER_ENGINE=none`). One `devstack::Node` runs either runtime behind the
node's front door (front.rs): celld listens on a port of its own, which
the front door is told, as it learns workerd's from wrangler's. Stop
(SIGINT), crash (the group SIGKILLed) and the e2e's signal handler serve
both.

Every binding and runtime API master's cell uses, against the fork's
`selfhost` line:

| Binding or API | On celld | Class | How the cell runs |
|---|---|---|---|
| `durable_objects`: Fragment, Principal, Registry, Ledger, SearchIndex, Vault | yes | works | |
| `durable_objects`: Computer | the class runs, its `ctx.container` not running | off | no computers: `FRAGMENT_COMPUTER_IMAGE` unset, so a computer is refused, typed ("this deployment makes no computers"); its container host finds no container running, so a person's wipe still empties their computer's rows |
| `durable_objects`: Migrations, and R2 `STAGE` | yes | off | the finite.computer move: unconfigured (`FRAGMENT_MIGRATIONS_MOVER` unset), as on any deployment but finite.chat's |
| `migrations` (`new_sqlite_classes`, v1 to v4) | yes | works | |
| `durable_objects.code_update_strategy` | accepted, no effect | works | celld reads the bindings alone; `celld dev` deploys once (`--no-watch`), and the e2e's reload restarts the node |
| `worker_loaders` (`LOADER`) and facets (`ctx.facets`) | yes | works | apps' facets, their databases capped past the cell's own (`CELLD_FACET_MAX_BYTES`), loaded code locked down (`CELLD_DYNAMIC_LOCKDOWN`); `facets.abort` releases a brain's facet (found 51, fixed in the fork) |
| `workflows` (`JOBS`) | yes, durable | works | a sleep is an alarm, so a job sleeping through a crash wakes (local workerd's does not) |
| `queues` (`DELIVERIES`, `METERS`, the dead letters) | yes | works | |
| An object's requests in flight | 64 at most by default | works, configured | devstack runs celld with 512 (found 50) |
| `r2_buckets` (`BLOBS`) | yes, `R2Bucket` | works | |
| `assets` (`ASSETS`, `run_worker_first`, `html_handling`, `not_found_handling`) | yes, `Fetcher` | works | |
| `secrets_store_secrets` | yes, `Fetcher`: read from the node's secrets backend | works | seam 12; people's secrets stay in the Vault object |
| `ai` (`AI`: Workers AI) | bound (`Ai`); every call rejects, "this node has no Workers AI" | degraded | text tiers on an OpenAI-compatible API (seam 3); a slot left on Workers AI, image steps, transcription and embeddings fail. Dev and the e2e keep the fake (`FRAGMENT_AI_URL`), and their local config drops `ai` on either runtime |
| `send_email` (`EMAIL`) | bound (`SendEmail`); every send rejects, `E_NO_MAILER` | degraded | a failed send: Mail, below. Dev and the e2e keep the mail fake (`FRAGMENT_MAIL_URL`) |
| `browser` (`BROWSER`) | bound (`Fetcher`); every fetch rejects, "this node has no Browser Rendering" | degraded | a card's shot fails, as one Browser Rendering fails. Seam 7 brings a renderer |
| `images` (`IMAGES`) | bound (`ImagesBindingImpl`); every output rejects, "this node has no Images" | degraded | an icon's making offers no candidate, and an upload that needs fitting fails (`host_failed`); a PNG at the icon's size is kept as it is |
| `containers` | accepted; a node with no engine (`CELLD_CONTAINER_ENGINE=none`) builds no image, and each object's `ctx.container` is there, not running, its start throwing "this node runs no computers" | off | celld's Docker engine has no egress intercepts or snapshots, which a computer needs; seam 2 places computers on sandcastle nodes |
| `ctx.exports` | yes (`DurableObjectState.exports`) | works | used only by a computer's container host |
| `interceptOutboundHttp`, `interceptOutboundHttps`, `snapshotContainer` | krun engine only | off | with computers (seam 2) |
| `connect()` (`cloudflare:sockets`) | yes | off | the XFTP relay is a computer's |
| `compatibility_flags`: `nodejs_compat` | partial (its `node:` modules) | works | the Sandbox SDK's imports load |
| `compatibility_flags`: `containers_pid_namespace` | accepted, no effect | off | with computers |
| `build` | refused | n/a | dropped by the render (`DROPPED`): worker-build has run (`cargo xtask build`) |
| A streamed request body forwarded to an object | its length carried, as workerd's (found 47, fixed in the fork) | works | |
| `CF-Connecting-IP` | not set | degraded | every CLI code's ask counts as one address, `unnamed`, unless a front proxy sets the header (found 48) |
| Cache API, HTMLRewriter, cron triggers | | n/a | the cell uses none |

- **Dev:** `cargo xtask dev --runtime celld` (`CELLD_BIN`; esbuild from
  worker-build's cache or `CELLD_ESBUILD`): the same fakes and settings as
  on wrangler, no computers, the secrets bound as on wrangler and read by
  the node from OpenBao.
- **The e2e:** `FRAGMENT_E2E_RUNTIME=celld`. Its rung (`needs::Rung::Celld`)
  has everything a local run has but the runtime's containers and the
  finite.computer move: a section that needs computers is a skip saying
  seam 2. Checks celld cannot make are skips saying why (a card's shot,
  an icon's making; found 31's 413); the check local workerd cannot make
  runs (a job sleeping through a crash). The build makes no computer
  images.
- **Master's path is unchanged:** nothing here branches on the runtime,
  and the cell has no code of celld's.

## 3. Models from a file

`cargo xtask dev` reads `FRAGMENT_MODELS_FILE`: a deployment's `models`
block alone, as its config writes it, checked as a deploy checks it
(`deploy::models_cell`) but for its URLs, which may be this machine's
(`http://127.0.0.1:<port>`, as a local fleet's egress reaches it). The key
its OpenAI-compatible slots name is the value in `FRAGMENT_MODEL_KEY_FILE`
(one; a server that takes none still gets a word). Slots it leaves out,
and image steps, stay on the Workers AI fake. It is `--real-agent`'s
models without our Hermes image, and the two are one or the other. A
self-hosted model names its price in its slot, zero for the box's own:

```jsonc
{
  "cheap":  { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "local-key", "model": "local-27b",
              "price": { "input": 0, "output": 0 } },
  "medium": { … the same }
}
```

```sh
FRAGMENT_MODELS_FILE=models.jsonc FRAGMENT_MODEL_KEY_FILE=model-key cargo xtask dev
```

## 12. Secrets: the store's bindings, read from the node's backend

The cell reads each deployment secret as `await env.<BINDING>.get()`
(cell/src/keys.rs, through workers-rs's `SecretStore`), on every runtime.
celld binds `secrets_store_secrets` as Cloudflare does: each a `Fetcher`
whose `get()` answers the secret, which celld's host reads by its
`secret_name` from the node's secrets backend, outside every isolate. The
backend is the node's configuration (celld's `CELLD_SECRETS_FILE`, an
owner-only JSON file), never the Worker's variables, and nothing falls
back to a backend it does not name (celld's docs/services/secrets-store.md):

- **`openbao`:** Vault's KV v2 API (OpenBao, or a company's own Vault):
  `GET <addr>/v1/<mount>/data/[<prefix>/]<secret_name>`, the value its
  field `value`, with the token in `token_file` (read at every read, so
  the token the stack renews or mints there is the one sent) and
  `X-Vault-Namespace` when `namespace` is set. A read takes 5 s at most
  and follows no redirect; a failure rejects, typed, never a hang
  (`SecretsError`, its `code` `missing`, `refused`, `sealed`, `down`,
  `failed` or `malformed`). celld caches nothing: keys.rs keeps a value a
  minute per isolate, as it keeps the store's.
- **`vars`:** the values in the file itself (devstack's `Secrets::Vars`):
  `celld dev`'s alone, for dev and the e2e. A node of a fleet refuses it
  at start.
- An unknown secret rejects (`missing`), as workerd's binding does; with
  no backend named, every read rejects (`unbound`).
- People's own secrets are not here: they stay in master's Vault Durable
  Object, sealed under keys derived from the host secret, as on
  Cloudflare.

**OpenBao is a self-hosted deployment's backend** (Paul, 2026-11-11): it
stands in for Cloudflare's Secrets Store, holding the platform's root
secrets (the host secret, the code store's key, sign-in's and the
connections' client secrets, the model keys, the operator keys).

**OpenBao** (crates/devstack/src/openbao.rs), used as it ships:

- **The pin:** 2.7.2 (2026-10-26), the newest 2.x on 2026-11-11: each
  platform's tarball by its SHA-256 in the release's `checksums.txt`, whose
  signature (`checksums.txt.gpgsig`, by subkey E617 DCD4 065C 2AFC 0B2C
  F7A7 BA8B C08C 0F69 1F94) checked good on 2026-11-11 against OpenBao's
  key (primary 66D1 5FDD 8728 7219 C8E1 5478 D200 CD70 2853 E6D0, fetched
  from openbao.org and identical to the release's copy). Fetched once
  into `target/tools`; `FRAGMENT_OPENBAO_TARBALL` names an intranet's copy,
  checked the same.
- **Its state:** a directory of its own (0700): `seal.key`, 32 random
  bytes (0600), the static seal's, so every start unseals itself
  unattended; `data/`, pebbledb; the stack's AppRole ids (0600).
- **Initialised once, by itself:** the config's `initialize` stanza runs on
  the first start alone: KV v2 at `fragment/`, the two policies, the
  cell's token role and the stack's AppRole; then it revokes its root
  token.
- **Seeded at each start:** what `Fleet::configure` would write to
  wrangler's local store goes to `fragment/<its store name>`, a new
  version only where a value changed.
- **Secret zero:** the cell's token, policy `fragment-cell`: `read` on
  `fragment/data/*` and nothing else. Periodic (30 days), renewed at each
  start and hourly while `xtask dev` runs, minted again when it has
  lapsed; in `cell.token` in OpenBao's state (0600), which the node's
  secrets file names by its path.
- **Loopback only**, no TLS: the node and OpenBao share the box.
- **Its audit log is on:** every request, its secrets HMAC'd by OpenBao,
  in `audit.log` beside its data (the e2e counts the cell's reads from it).
  A deployment rotates it as it rotates any log.
- **Its unseal key** is the static seal's key file, `seal.key` in its
  state directory (0600, beside its data): whoever reads that directory
  can unseal it. The hardening path, not built: a TPM-held key, a PKCS#11
  HSM, or a cloud KMS auto-unseal, each keeping the key off the disk that
  holds the data.
- **Choosing it:** the default on celld. `cargo xtask dev --runtime celld`
  runs it (on :8802, its state in `target/devstack/openbao-8802`, which
  `--clean` clears) unless `FRAGMENT_SECRETS_BACKEND=vars`; the e2e on
  celld unless `FRAGMENT_E2E_SECRETS=vars`. `wrangler dev` keeps its own
  store.
- **A company's Vault:** the node's secrets file names it (`{"backend":
  "openbao", "addr": "https://vault.corp.example:8200", "mount": "secret",
  "prefix": "fragment", "namespace": "eng", "token_file": "…"}`), each
  secret a KV v2 entry whose field is `value`, its CA in
  `CELLD_EXTRA_CA_FILE`. Only OpenBao is tested.

## Mail

Master mails through Email Sending's binding (`EMAIL`), or in dev and the
e2e through the fake. celld binds `EMAIL` with no mailer behind it: every
send rejects, `E_NO_MAILER`, "this node has no mailer", and the cell takes
it as any refused send (`upstream_failed`). An invite to someone new is
refused (its invite is not kept), as before; a key's mail and an export's
are logged as failed; an org's `mailed` says false. Whether a box with no
mailer should still make invites is a product rule, for a PR of its own.
celld's mailer (SMTP relaying) is a later change. Debt:
docs/technical-debt-ledger.md, "No mail without Email Sending".

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
    Both runtimes' stacks share that file, so a celld state that outlived
    it is refused at any dev start, on either runtime
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

- 35, celld's R2 and assets bindings' classes read as `Object`, which
  workers-rs's typed reads refuse: the fork names every binding's class as
  workerd does, so the cell reads them as it does on Cloudflare.
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
| Voice transcription (iOS voice memos too) | Whisper on Workers AI | fails: celld's `AI` rejects ("this node has no Workers AI"); a later seam moves it to OpenRouter |
| Job steps `ai.image`, `ai.video`, `ai.decide`; embeddings | Workers AI (video through AI Gateway too) | fails, as above; a later seam |
| Drawn icons, and fitting uploaded ones | Workers AI and Cloudflare Images | a making offers no candidate, a fitting fails (`host_failed`): celld's `AI` and `IMAGES` reject; a 256×256 PNG is kept as it is |
| Preview cards | Browser Rendering | a shot fails: celld's `BROWSER` rejects; seam 7's renderer |
| Platform email | Email Sending | a failed send: celld's `EMAIL` rejects (`E_NO_MAILER`); an invite to someone new is refused (debt) |
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
4. Secrets: OpenBao (or the org's own Vault), its unseal key kept
   safe (a TPM, an HSM or a KMS once built); the host secret, 32 or more
   random bytes, among the platform's root secrets there (seam 12).
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
# its secrets in its node's file instead of OpenBao (`celld dev`'s alone)
FRAGMENT_SECRETS_BACKEND=vars CELLD_BIN=… cargo xtask dev --runtime celld
# the e2e on celld (its node reads OpenBao unless FRAGMENT_E2E_SECRETS=vars)
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… cargo xtask e2e --only auth,create,…
```

## Not yet

- Computers (seam 2): none on celld. devstack's node has no engine
  (`CELLD_CONTAINER_ENGINE=none`); celld's Docker engine has no egress
  intercepts or snapshots, which a computer needs; its krun engine
  (sandcastle's) has both.
- Sign-in by OpenID Connect (seams 4, 9).
- Own hardware in points (10), cards (7), macrofiche (5).
- `celld deploy`: no xtask path yet; this is the dev stack and the e2e.
