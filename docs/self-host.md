# fragment self-hosted

Status: **re-ported 2026-10-22**, by seam, onto master (d8e0f86b) on the
branch `selfhost-port`. The spike it replaces (`selfhost`, draft #133: 72
commits on master of 2026-10-06, its design this doc's 2,271 lines there)
is the reference implementation: `git log 923bb5cd..origin/selfhost`. No
rebase: each seam is ported onto master's shapes, one commit each, and the
old branch is force-moved to the result at the end (Paul, 2026-10-22).
Where this disagrees with `docs/cloudflare-v1.md`, that doc wins for
master.

The first milestone is still Paul's home: the stack on this box, run as an
intranet would run it, that he can demo from his iPhone. A company's own
requirements come after. **Every seam is ported** (2026-10-22): seam 9, the
LAN mode, last, and its guide (docs/self-host-lan.md) rehearsed end to end
from this box, short of sudo, the router and Paul's devices.

## The four ways fragment runs

1. **On Cloudflare.** Master's plan of record.
2. **Self-hosted on bare metal.** celld runs the cells and sandcastle runs
   the computers, with state in a bucket (or on disk, for one box).
3. **A company's network, with no internet.** The model is on the same
   network. A developer's box is the smallest case.
4. **Blends.** Computers on sandcastle nodes with everything else on
   Cloudflare is the first worth selling.

## The rule: one cell, interfaces, placement as configuration

The cell is written against Cloudflare's runtime APIs (Durable Objects,
the Worker Loader and Facets, Workflows, Queues, R2, `ctx.container`) and
protocols for what sits outside a runtime (OpenAI's chat completions,
OpenID Connect, the code-store contract, the DevTools Protocol, Web Push).
Self-hosting supplies other implementations of the same interfaces. The
cell never asks "am I self-hosted?": it reads what it has from its
configuration and its bindings. A mode flag is a design bug.

## What master made redundant

- **Models (the spike's seam 3).** Master's route runs any slot on an
  OpenAI-compatible API, `{"provider": "openai", "base_url", "key",
  "model", "body"?}` (decision 61; deploy/example.jsonc). A self-hosted
  model is one more such slot, so the spike's `FRAGMENT_MODEL_URL` and its
  tier map are cut, never ported. What dev needed was to read a models
  config: `FRAGMENT_MODELS_FILE` (seam 3, below).
- **Billing.** Off where no Stripe key is bound (no `stripe` in the
  config): seats are an operator's comps, and nothing is sold.
  `FRAGMENT_DEFAULT_PLAN` still names a new person's plan (a company's:
  `seat`).
- **Secrets.** Master moved the deployment's secrets into Cloudflare's
  Secrets Store (#134), which celld lacks: seam 12's stand-ins.
- **Agents.** goose's agents' Worker went (master's computers run Hermes),
  and with it the spike's second Worker on celld (`--with`, agent/celld.mjs).

## The seams

| # | Seam | Status |
|---|---|---|
| 1 | The runtime: celld | **done** (below) |
| 2 | Computers on sandcastle nodes: placement, the uplink, bring your own computer | **done** (below) |
| 3 | Models | master's; dev's models file **done** |
| 4 | Sign-in: OpenID Connect | **done** (below): beside WorkOS, on master's npub/email identity |
| 5 | Files: macrofiche, the code-store contract | **done**: the store by configuration; macrofiche#1 and #2 open |
| 6 | Blobs | nothing to port: celld's R2, over its bucket or `celld dev`'s disk |
| 7 | Preview cards: the renderer over chrome-headless-shell | **done** |
| 8 | Notifications | design only: push is off offline |
| 9 | Hosts and TLS: the LAN mode (`--lan`) | **done** (below); its guide rehearsed |
| 10 | Money: who used what, in units (own hardware among it) | **done** (below) |
| 11 | Nothing calls home | design only |
| 12 | Secrets | **done**: the stand-in, OpenBao behind it |
| — | Mail | debt (docs/technical-debt-ledger.md, "No mail without Email Sending") |

### 1. The runtime: workerd or celld (done)

celld (github.com/futurepaul/celld, its `selfhost` line: upstream's
release and the fork's parity fixes) runs the cell from a config rendered
for it: `wrangler.celld.jsonc` beside the cell's own
(`crates/devstack/src/celld.rs`), less what celld has no part in. One
`devstack::Node` runs either runtime: celld's `start` builds its command,
and stop (SIGINT), crash (the group SIGKILLed) and the e2e's signal
handler serve both.

Every binding master's `cell/wrangler.jsonc` uses, against celld (v0.6.2 and v0.6.3 alike):

| Binding | On celld | How the cell runs without it |
|---|---|---|
| `durable_objects`, `migrations` | yes | |
| `worker_loaders` (apps' facets) | yes | |
| `workflows` (jobs) | yes, durable: a sleep is an alarm, so it outlives a crash (local workerd's does not) | |
| `queues` (deliveries, meters) | yes | |
| `r2_buckets` (blobs) | yes; its class reads as `Object` | read by what it does (found 35) |
| `assets` (the release's files) | yes, with `run_worker_first`; its class reads as `Object` | read by what it does (found 35) |
| `containers` (computers) | accepted (experimental, on Docker), dropped by the render | no computers: `FRAGMENT_COMPUTER_IMAGE` unset. A self-hosted deployment's are a sandcastle node's (seam 2) |
| `ai` (Workers AI) | refused | each slot on an OpenAI-compatible API (seam 3); a slot left on Workers AI fails its calls, and image steps are off. Dev and the e2e keep the fake (`FRAGMENT_AI_URL`), a URL |
| `browser` (Browser Rendering) | refused | no cards: a live is skipped, said once (`card.skipped`, `no_browser`), after the owner's ledger is asked as before. Seam 7 brings the renderer |
| `send_email` (Email Sending) | refused | no mail: each send logs `mail.unsent` ("email not sent: this runtime has no mailer, not yet implemented") and answers no id, never an error; invites and sign-in go on. Debt |
| `secrets_store_secrets` (devstack adds it) | refused | the cell's stand-ins (seam 12) |
| `build` | refused | worker-build has run (`cargo xtask build`) |

- **Dev:** `cargo xtask dev --runtime celld` (`CELLD_BIN`; esbuild from
  worker-build's cache or `CELLD_ESBUILD`): the same fakes and settings,
  the secrets in `.dev.vars` behind the stand-ins, no computers. It keeps
  the mail fake, as dev on wrangler does.
- **The e2e:** `FRAGMENT_E2E_RUNTIME=celld`. Its rung (`needs::Rung::Celld`)
  has everything a local run has but the runtime's containers: `computers`,
  `chat`, `shell-ui` and `hermes` are skips saying seam 2. Checks celld
  cannot make are skips saying why (a card's shot); the check local
  workerd cannot make runs (a job sleeping through a crash). The build
  makes no computer images. The restart section ends on a node with no
  mailer (`Shape::NoMailer`): an invite to someone new, their first
  sign-in, and the warnings in the node's log.
- **The bucket:** celld takes a cell's ownership with a conditional write:
  the bucket needs `If-None-Match` and `If-Match` (S3, R2, Ceph RGW, Tigris;
  not Garage). One box needs none: `celld dev` keeps state in
  `.celld/dev`.
- **Master:** two reads by what a binding does, a card skipped where there
  is no browser, mail that says it is not sent. Nothing branches on the
  runtime.

**Evidence:** see "Re-ported, 2026-10-22", below.

### 2. Computers on sandcastle nodes (done)

A computer's container is `ContainerHost`'s (`cell/entry.mjs`), and every
runtime call it makes, and every call the Sandbox SDK's `DirectoryBackup`
makes, is a method of one object: `ctx.container` (Cloudflare's, or
Docker's under `wrangler dev`), or, when the deployment lists sandcastle
nodes (`FRAGMENT_NODES`), a `NodeContainer` (`cell/node.mjs`) over the
node's API (sandcastle's docs/node.md), each call signed with the node's
secret. computer.rs, the restore gate and the images do not know which.
celld's render drops `containers` (seam 1): on celld, nodes are the only
computers.

**Master's calls, each checked against the node** (a start's options as
master passes them): `start({env, enableInternet, instance,
image | containerSnapshot})`, the instance its custom size (2 vCPU, 6 GiB,
12 GB: the engine's `Instance::Custom`); `monitor()`; `exec(argv,
{stderr: "combined", signal})` (an abort is a SIGKILL frame) and the SDK's
piped stdin and stdout; `signal(n)`; `destroy(reason)`;
`getTcpPort(port).fetch` (HTTP and WebSocket); `snapshotContainer({name})`;
`interceptOutboundHttp(s)` (the api and model hosts, the platform's own
hosts by a glob, `*.<zone>`: seam 9, found 40, and on a plain-HTTP
platform's port `*.<zone>:<port>` too, a node's own form; the swap's hosts, and
`DirectoryBackup`'s gateway, set again by each save: a binding replaced,
never a new intercept); and `setInactivityTimeout`, nothing on a node (the
Computer's own sleep stops it). Snapshots, master's cache of the save, are
node-local and keyed by the image's reference, as everywhere; a computer
is pinned to its node for life, so its snapshots are always there.

**Reach.** A node either listens (`"url"`), or dials the platform's
`/api/nodes/uplink` (`"uplink": true`, S7) and one `Node` Durable Object
per node holds the socket (`cell/uplink.mjs`, unchanged from the spike:
calls are one message each, found 9; a dropped `GET` is asked again on the
next dial, found 10). Intercepts come back to `/api/nodes/egress`, signed,
and go to the computer's object, which checks them again. `Node` is the
one object that runs no Rust and checks no caller's release: its calls are
the node's protocol (xtask's guard counts it apart).

**The list** (`FRAGMENT_NODES`, checked by `fragment_core::placement` at
the first request):

```json
{ "nodes": [
    { "id": "box", "url": "http://192.168.50.7:9400", "arch": "x86_64", "capacity": 16 },
    { "id": "mac", "uplink": true, "arch": "aarch64", "capacity": 8 } ],
  "images": {
    "stub":   "docker.io/library/fragment-stub@sha256:<64 hex>",
    "hermes": { "x86_64": "registry.lan/fragment-hermes@sha256:<64 hex>" } } }
```

A node's secret is `FRAGMENT_NODE_SECRET_<ID>` (the id upper-cased, `-` as
`_`): a Worker secret, or a Secrets Store binding of that name (its
`get()`, so seam 12's stand-ins serve it), never in the list. An image has
one reference for every architecture, or one per architecture (arm64 is
the Mac's, later).

**Pinned by digest.** Master pins a deployment's images by registry digest
(docs/computers.md, "Stable and canary"); a node's are pinned the same
way, and must be: a computer's version is its reference's digest, and a
snapshot is reused only under the reference it was taken of, so a tag
rebuilt under the same name would start the old image's snapshot and
never tell an update. Two ways a node holds one:

- **Pulled**: `name@sha256:…` from a registry the node reaches; the engine
  checks every digest it pulls.
- **Loaded** (`cargo xtask node-images`, and the e2e's real engine): the
  image built here, `docker save`d, loaded over the engine's socket, and
  named `docker.io/library/fragment-<name>@<digest>` by the digest the
  load answers (the OCI manifest of its config and layers as loaded, not a
  registry's). sandcastle's `node-digest-refs` (off `node`) resolves a
  reference pinned by a digest the node holds to that image, whatever name
  it came under; a rebuild is a new reference.

The Docker double (two nodes) runs local Docker's tags
(`fragment-<name>:e2e-<run>`, one per run): Docker knows no digest for an
image built here.

**Placement** (`computer.rs` `place`, at a computer's first start): every
node probed at once (health within 6 s, the architecture it reports, its
object's count); of those up, reporting their listed architecture and
holding the image for it, the fewest computers for its capacity wins, a
tie to the one listed first; the node's object takes it once and refuses
one past capacity, so a race never overfills it. **Pinned for life**
(`node` and `placed` in its view; moving one is not built). A node that
does not answer is a typed answer within a bound, never a hang: the wake
answers 503 `node_down` naming it (and its view's `why` says so until a
start comes up), 503 `no_node` with each node's reason when none can take
a new computer, and 410 `node_revoked` for a person's own node they
revoked. A start that failed for its node is no snapshot's failure.

**A node silent under an awake computer** (found 39): silence is no exit.
A computer's watch is its node's `wait`; cut (the node's process
restarting, a blip, its uplink dropping), the node is asked whether the
container still runs, and a node that does not answer is asked again, the
pause doubling from 1 s to 30 s, for as long as it is silent
(`NodeContainer`'s `#look`). When it answers, a container still running
is waited on again (no restart, no rollback), and one gone is an exit.
Silent past **two minutes** (`SILENT_MS`), the computer says its node is
down: its view's `why` names it, and a wake answers 503 `node_down`
(`computer/node`, its watch's report); nothing else changes, and it goes
on by itself when its node answers. A new isolate whose node is silent
takes the container as running (`presume`), and its watch decides the
same way; a port's request to a silent node is `node_down` too. Why two
minutes: longer than every outage a node heals by itself (its process or
its engine restarting, seconds, the engine keeping its VMs; an uplink that
lost its connection, which the node notices within its `SILENT_MAX`, 60 s,
and dials again within its `DIAL_WAIT`, 15 s), and short enough that the
owner hears within two minutes, not after an awake computer that answers
nothing. A report changes only what the computer says, so one made just
before the node answers costs a note that clears itself. sandcastle needs
nothing for it: `sandcastle-node` keeps no state of a container (every
call goes to the engine's socket, and the handler socket the engine dials
for each intercept is bound again at the same path), so a node restarted
in front of an engine that kept its VMs answers `wait`, `inspect` and
`exec` for the containers started before. An uplink's `GET` cut by its
node's restart is asked again on the next dial (found 10), so its `wait`
is not even cut; the first run found it asked again on the very uplink
being lost (celld still reads one open as its close is heard), where it
waited for good, so the computer never heard of the outage, nor of an exit
after it: `Uplink` never asks again on the one it lost. Still true, a
decision for Paul: an idle computer whose
node stays down tries to sleep after its 20 minutes; its saves fail, and
past the unsaved bound (30 minutes) it sleeps unsaved, as any computer
whose saves keep failing does, so its next wake goes back to its last
save.

**Bring your own computer** (experimental; `FRAGMENT_BYOC=on`, off unless
said): `sandcastle-node pair <platform>` starts a device authorization
(`POST /api/nodes/pair`, unsigned; RFC 8628's shape,
`fragment_core::pairing`); the person approves its 8-letter code at
`/nodes/pair` in their session, from the platform's own origin (ticking
"Run my new computers on it" chooses it at once); the node's next poll
takes the id the platform gave it (`paired-<16 hex>`) and its secret,
once, sealed in the registry for that id (its tables: the Registry's step
`0005_paired_nodes`). A person's node runs their computers only when they
chose it (Settings → Computers (experimental), `PUT /api/nodes/prefer`);
a choice that cannot take a computer falls back to the deployment's rule,
saying why in `placed`. Revoking (`DELETE /api/nodes/{id}`) drops its
secret, cuts its uplink (4003) and answers its every call and dial
`node_revoked`. The bounds are the spike's (10-minute codes, 32 waiting,
10 a minute, 5 wrong codes, 8 live nodes, live names apart).

**sandcastle's part** (its `node` branch is what this box runs): the
engine resolving a digest a node holds is `node-digest-refs` (one commit
off `node`, pushed, not merged: Paul's), installed on this box's engine
(the binary before it kept beside it as `sandcastle-engine.before-digest-refs`).
The two-node runs use `node`'s own `sandcastle-node`, double and relay.

**Evidence, 2026-10-22** (celld: the fork's `selfhost` line on v0.6.3;
OpenBao and the renderer on, as the celld e2e has them):

- **`cargo xtask check`:** 803 passed, 0 failed.
- **Two nodes on celld** (`FRAGMENT_E2E_NODES=two`; computers, chat,
  shell-ui, placement, pairing, restart, billing): **757 passed, 0
  failed, 1 skipped** (the stub's awake save, master's own skip). Among
  them: each kind of node takes a computer, its container killed under it
  comes back by itself, each node down answers `node_down` within 15 s,
  pairing end to end with `sandcastle-node pair`, own hardware in points
  read by an operator, a computer on a node across a restart and a crash,
  and billing's four wipe checks (a wipe's computer step had no container
  on celld before).
- **The real engine** (`FRAGMENT_E2E_NODES=real`, one node in front of
  this box's engine, every computer a jailed microVM, the images named by
  digest; computers, chat, hermes): **428 passed, 0 failed, 1 skipped**
  (the same). Its first run found the stale `wait` clearing a new start's
  `running` (22 failed, all of it); fixed in `NodeContainer` (a start's own
  number).
- **Found 39** (two nodes on celld, `--only placement`, the Docker
  double): **23 passed, 0 failed**, eight of them new (`outage`, both
  nodes at once): each node's process restarted under an awake computer
  (the same start, the file it wrote kept); down past two minutes (its view
  names its node, a wake answers 503 `node_down`, the same start); back
  (it goes on by itself, its file and screen there, no rollback); and its
  container watched as before after (killed under it, started again). Its
  first runs found the uplink's lost `wait` (above). With it, computers
  and restart on two nodes: 247 passed, 0 failed, 1 skipped (a run before
  had 3 failures, one flake: a container that exited in the fail-saves
  check, 39 s before its bound; not seen again). `cargo xtask check`: 835
  passed, 0 failed. The real engine's run of it waits: Paul's own
  computers are on this box's engine.
- **`cargo xtask node-images`:** the stub and Hermes built, loaded and
  named `docker.io/library/fragment-<name>@sha256:…`; the stub's digest
  the e2e's for the same content (`a92db44a…`), Hermes' found built.
- **Master's path, `wrangler dev` with local Docker** (computers, chat):
  **367 passed, 0 failed, 2 skipped** (the stub's awake save, and the
  killed container's check, which needs a node the run started).

- **After the timing work** (`selfhost-perf`: master's restart fix, the
  restart waits gone, a start's intercepts in one call):
  - **Two nodes on celld:** computers 197 passed, 0 failed, 1 skipped.
    Chat, shell-ui, placement, pairing and restart had 0 failed in a run
    whose computers section ended when celld fenced itself (found 45).
  - **Master's path:** computers and chat, 367 passed, 0 failed, 2
    skipped.
  - **Without the race fix:** two nodes' computers with the waits gone
    had 190 passed and 7 failed (each read a restart, or what came after
    one).

**The self-hosted cell's cost:** this seam adds 226 KB to the cell's Wasm
(6,471,209 bytes with seams 4, 5, 7 and 12, to 6,697,133), the router's
and the Registry's new routes most of it, past master's 6.5 MB. The
lane's budget is 6.7 MB (`CELL_WASM_MAX_BYTES`, a commit of its own; the
debt ledger's Wasm-budget entry): 2.9 KB left.

**What celld needed, found again on master's computers:**

- **found 21**: an exit's report (and a tab's) comes back in through the
  object's namespace, a request of its own, never as a continuation of
  the call that started the container (celld ties a WebSocket to its
  request: denoland/celld#254). Master reported through `this.rs`.
- **found 22**: a node's `wait` outlives celld's 120 s fetch bound: a cut
  poll asks whether the container still runs, and waits again
  (`#waited`).
- **found 20**: every socket path the harness hands a node fits
  `sun_path` (`sandcastle::socket_path`).
- **New, the lanes' timing on celld with nodes** (master's lanes, first
  run there): a reply's push run (a trigger's Workflow, on celld a cell of
  its own) can end after its chat's page closed, so the chat lane waits
  for every reply's run first; a sleep through a node is slower, so the
  always-on check waits for a sleep under way (its owner's sleep then
  answers `sleeping`, as it should). An owner's restart answered
  `starting` while its container's exit report (found 21's own request)
  made the start: that was master's race, fixed there (#518: the exit
  report during a sleep's stop starts nothing, the stop's own end does),
  so the restart checks and the shell's restart notice read the
  restart's answer again, as on master ("Where the time goes", below).

### 3. Models: master's slots, and dev's models file

`cargo xtask dev` reads `FRAGMENT_MODELS_FILE`: a deployment's `models` as
its config writes them, checked as a deploy checks them
(`deploy::split_models`) but for its URLs, which may be local (dev's
egress is). The key the OpenAI-compatible slots name is the value in
`FRAGMENT_MODEL_KEY_FILE` (one; a server that takes none still gets a
word). Slots it leaves out, and image steps, stay on the Workers AI fake.
Bonsai on this box, every tier and the fallback:

```jsonc
{
  "cheap":    { "provider": "openai", "base_url": "http://bonsai.localhost/v1", "key": "bonsai-key", "model": "bonsai-2-27b",
                "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } },
                "stale_timeout_s": 900 },
  "medium":   { … the same },
  "fallback": { … the same }
}
```

- **Its URL** is Caddy's name for it, `http://bonsai.localhost/v1`: the
  route takes https on a host name, or on a fleet whose egress is local
  http to this machine by any local name, a loopback address,
  `localhost` or a name under `.localhost`, on any port or none
  (`catalog::Plain::Local`; found 36, which Paul answered yes,
  2026-10-22). celld and workerd both reach such a name on this box
  through its resolver (systemd-resolved answers `*.localhost` with
  loopback): the platform resolves nothing itself.
- **Its price** is its slot's ceiling, zero: a self-hosted model has no
  list price, and master prices a model the book lacks only from
  `body.provider.max_price`, OpenRouter's field, which goes to the server
  with each call (NInfer ignores it; found 37).
- **Its patience** is 900 s (`stale_timeout_s`, a tier's or the
  fallback's; found 42, and master's draft #527): how long an agent's
  call may stream nothing before Hermes gives up on it. Without it, our
  image's 20 s for cloud models kills a call that a local server is
  silent on for longer: a tool call it sends in one piece at its end (as
  NInfer did), a long prompt's prefill, or its turn behind another caller
  on the one GPU (found 42). 900 s is Hermes' own patience for a server
  it knows is local (`agent.local_stream_stale_timeout`), which behind
  the computer's intercept it cannot know. Leaving it out is not Hermes'
  own patience: the image's 20 s stands. Hermes' cloud default (180 s,
  240 s past 50,000 tokens of context) would cut a long write on a busy
  box, and a kill on a deployment's one server only asks it again. A
  hung server still ends the turn, after 900 s and the one more try a
  stalled call gets.
- **No fallback to itself.** With every slot on one model, an agent has
  no fallback: its profile names none, since switching would only ask
  the same server again (the route's own fallback skips the same
  upstream too).

The e2e takes the same file as `FRAGMENT_E2E_MODELS_FILE` (its key's in
`FRAGMENT_E2E_MODEL_KEY_FILE`): its agents are then real ones (needs.rs
`Rung`'s `real_agent`), and the run is `agent-app` alone, which asks a
Hermes on that model for a shared grocery list and opens what it made in
Chrome (every other section scripts the model).

**Evidence:** see "Re-ported, 2026-10-22".

### 4. Sign-in: OpenID Connect (done, beside WorkOS)

Any OpenID Connect provider signs people in: Dex on the LAN, Keycloak,
Authentik, ADFS, Okta, Entra. It is master's sign-in with another
front door, chosen by configuration: with `FRAGMENT_OIDC_ISSUER` set, the
provider signs people in; without it, WorkOS does, as master has it.
WorkOS bound beside it serves Pipes' connections alone (a person signed in
by the provider has no WorkOS user, so no connections). The spike's
AuthKit unification (WorkOS as one more issuer) is not ported: master
keeps WorkOS's path (Paul, 2026-10-22).

**Who a person is (decisions 45 to 50).** A sign-in is `(issuer URL,
sub)`; the first one makes a person, named by the npub of a key the
registry makes and seals for them, exactly as WorkOS's does (`person_for`,
the same function). The email is their handle: it must be there and
verified, it names one person (another person's is refused, 409, never
merged), the latest sign-in's is the one shown, and the invites waiting on
it are met at sign-in. A key-first identity (a CLI's key with no provider
account) is untouched: sign-in only gives browsers sessions, and a CLI's
key joins a person by the code they type at `/cli`, whichever provider
signed them in.

| Setting | What |
|---|---|
| `FRAGMENT_OIDC_ISSUER` | the issuer URL, exactly as its id_tokens' `iss` (https; http only on loopback). Its metadata is at `<issuer>/.well-known/openid-configuration` |
| `FRAGMENT_OIDC_CLIENT_ID` | the client |
| `OIDC_CLIENT_SECRET` | a Secrets Store binding, read only by keys.rs; on celld through the stand-in (seam 12; dev binds the secret `fragment-oidc-client-secret`). Unbound: a public client, PKCE its only proof |
| `FRAGMENT_OIDC_SCOPES` | default `openid email profile`; `openid` required |
| `FRAGMENT_OIDC_CLAIMS` | JSON, `{"email": <claim>, "vouched": <bool>}`; default `{"email": "email", "vouched": false}` |
| `FRAGMENT_OIDC_AUTH` | `client_secret_basic`, `client_secret_post` or `none`; default, with a secret, the first of the two the provider lists |

At the provider, the client's redirect URI is `<platform>/auth/callback`
and its post-logout redirect URI `<platform>/`. A setting out of shape
stops the isolate at its first request, saying which.

**The email rule.** An id_token's `email_verified` must be `true`, or the
deployment says the provider vouches for the claim it names: a company's
own directory, whose addresses are its to give (`"vouched": true`). Only
`email` has a verified flag, so another claim is taken only vouched for:
ADFS, `{"email": "upn", "vouched": true}`; a single-tenant Entra app,
`upn` likewise (an optional claim there). Never vouch for a provider whose
people choose their own addresses (a multi-tenant or social one). An
absent email, an unverified one, or a value that is no address signs no
one in: 403, on a page that says what the person can do, and the node's
log says why for the operator (`signin.refused`, naming the claim and the
setting).

**The flow.**

- **The request.** The router reads the provider's metadata, then the
  registry begins the sign-in: a state (bound to the browser by
  `Trip::SignIn`'s cookie, as WorkOS's), a PKCE verifier sealed for that
  sign-in alone, and a nonce. The browser goes to the
  `authorization_endpoint` with the S256 challenge and the nonce, and the
  `login_hint` when there is one. "Use another account" (`/auth/switch`)
  adds `prompt=login`: the provider asks who is signing in even holding a
  session (WorkOS's held-session dance has no place here).
- **The callback.** The registry spends the sign-in first, in one
  statement, before anything is awaited (`oidc::TAKE_LOGIN`): a callback
  sent again, or two raced, finish it once. It exchanges the code
  form-encoded with the verifier (RFC 6749 4.1.3), the client
  authenticated with Basic (id and secret form-encoded first, 2.3.1) or in
  the form; no redirect is followed, and the answer is read for its
  id_token only.
- **The id_token** (`fragment_core::oidc::verify`): its signature by a key
  of the provider's JWKS only, RS256 or ES256; never `none`, an HMAC, an
  algorithm the provider does not list, a `crit` header, an access token,
  or a key the token names. The rules choose the key and check its size
  and the signature's length; the arithmetic is the runtime's WebCrypto
  (`js::signature_verifies`: `crypto.subtle.importKey` of a JWK the rules
  make from the key's numbers alone, then `verify`, RSASSA-PKCS1-v1_5 or
  ECDSA P-256 over SHA-256), the one seam in the rules, which the host
  tests stand in for with the `rsa` and `p256` crates (dev-dependencies
  only). Then `iss` exactly, `aud` (and `azp` when there are
  several audiences), `exp`, `iat` and `nbf` within two minutes of skew,
  an `iat` at most 15 minutes old, the nonce, and `sub`. A token that fails
  is 401, on a page, logged as `signin.refused` with why.
- **Caches.** The metadata and the JWKS are kept an hour per isolate. A
  key the JWKS lacks fetches it again, at most once in 15 seconds: a
  rotation is picked up within that, and a provider naming keys it never
  published is not asked again and again.
- **Logout.** RP-initiated (RP-Initiated Logout 1.0) where the provider
  advertises an `end_session_endpoint`: the session's id_token, kept sealed
  in the registry, is the `id_token_hint` (Okta and ADFS require one), with
  `client_id` and `post_logout_redirect_uri`, left as a page as WorkOS's is
  (the shell's CSP holds a form's redirects). Otherwise (Dex) logout ends
  the sessions here only.
- **A restart mid-sign-in** finishes: the state, verifier and nonce are the
  registry's rows, and the metadata and keys are fetched again.

**What it shares with WorkOS's path, and why.** Shared, because it is the
same thing and sharing is less code: the round trip's cookie binding
(`auth::trip_begun`, `trip_bound`: a callback finishes only in the
browser that began it), the registry's pending sign-ins (`logins`: their
cap, their sweep, `begin` itself), the person (`person_for`: the npub, the
email's uniqueness, a wipe in progress), the session and its cookie,
invites met, logout's revocation, the pages. Apart, because they differ:
the redirect to the provider, the code's exchange (keys.rs:
`oidc_token`), the id_token's verification and the provider's caches
(`cell/src/oidc.rs`, `fragment_core::oidc`), logout's provider leg, and
the fake. WorkOS's code is unchanged but for a dispatch line at the top
of `begin`, `callback` and logout's provider leg in auth.rs, and `Begin`
and `LoggedOut` gaining a field each (WorkOS's carry none).

Where it lives: the rules in `crates/core/src/oidc.rs`; the router's half,
the metadata, keys and verification in `cell/src/oidc.rs`; the exchange
in `cell/src/registry/oidc_signin.rs`; `begin` and logout in
`registry/signin.rs`; the secret and the token request in
`cell/src/keys.rs`; the configuration in `cell/src/config.rs`
(`OidcConfig`); the fake in `crates/fakes/src/oidc.rs`. The registry's
schema step `registry/0004_oidc_signins.sql` adds `logins.verifier`,
`logins.nonce` and `sessions.id_token` (all NULL on WorkOS's rows); at
landing on master it takes master's next number.

**Private CAs.** A company's browsers trust its CA already. The cell's own
fetches (metadata, JWKS, token) go through the runtime's TLS: celld trusts
the operator's roots with `CELLD_EXTRA_CA_FILE` (the fork); `wrangler dev`
hands workerd `NODE_EXTRA_CA_CERTS`. On one box the issuer may be plain
http on loopback, as the fake and Dex are here.

**Offline providers.** Dex, Keycloak and Authentik serve discovery and a
JWKS and need no internet; so does ADFS 2016 and later, with `upn` for the
email. Entra needs the internet. SAML and LDAP go through a bridge the
company already runs (Keycloak, Authentik, Dex), not into the cell.

**Dev:** `FRAGMENT_SIGNIN=oidc cargo xtask dev` signs people in through
the fake on :8802 (any email); a real provider is `FRAGMENT_OIDC_ISSUER`
with `FRAGMENT_OIDC_CLIENT_ID`, `FRAGMENT_OIDC_CLIENT_SECRET_FILE` and the
optional settings. `FRAGMENT_DEV_PORT` moves the stack and its fakes, so a
second stack runs beside one on :8790. **The e2e:**
`FRAGMENT_E2E_SIGNIN=oidc`: every section that signs people in goes
through the fake; the signin section's WorkOS checks have OpenID Connect
twins, and its own (`oidc_cases`).

**Not built:** admins from a groups claim (operators stay
`FRAGMENT_OPERATORS`); `private_key_jwt`, the userinfo endpoint, refresh
tokens, back- and front-channel logout; OpenID Connect in `xtask deploy`'s
config (Cloudflare's deploy binds no `OIDC_CLIENT_SECRET`); a deployment
moving from WorkOS to a provider keeps nobody (each email is a
WorkOS-signed person's, and is refused to the new issuer: decision 45).
The platform's session lasts 30 days whatever the provider says, so a
person disabled at the provider keeps theirs until it ends or they sign
out (Open questions).

**Costs.** The cell's Wasm grew 127 KB (6.34 MB to 6.47 MB): the
verification code and its JSON, and `num-bigint-dig` (27 KB) under `rsa`.
The signature's arithmetic then went to the runtime's WebCrypto
(2026-10-22): the `rsa` crate left the cell, and with it `p256`'s
verification, 42,683 bytes (6,711,722 to 6,669,039 at this branch's tip
then; 130,961 bytes of `CELL_WASM_MAX_BYTES` left). `p256` stays for
code.storage's tokens and web push.

**Evidence, 2026-10-22** (celld: the fork's `selfhost` line on v0.6.3):

- **`cargo xtask check`:** 756 passed, 0 failed (14 new: 13 in
  `fragment_core::oidc`, valid tokens, bad signatures, algorithm
  confusion (`none`, HS256 keyed by the secret and by the RSA key's
  bytes), each wrong claim, a replay into the next sign-in, a rotation and
  the refetch bound, discovery, the token request, PKCE (RFC 7636's
  vector), the email rule, and a sign-in begun before a restart finishing
  after it once; and the fake's, the cell's half of the flow on those
  rules).
- **The e2e on celld through the OpenID Connect fake**
  (`FRAGMENT_E2E_SIGNIN=oidc`; auth, identities, signin, members,
  restart): **328 passed, 0 failed, 2 skipped** (seam 2's computers).
  Members' invites by email are met at a first sign-in through the
  provider; every provider check of the signin section passes (WorkOS's
  twins, in Chrome too, and its own): the eight spoiled id_tokens each 401
  on a page, a rotation picked up within the cooldown.
- **The same sections through the WorkOS fake, unchanged:** on celld 325
  passed, 0 failed, 2 skipped; on `wrangler dev`, master's path, 324
  passed, 0 failed, 3 skipped (local workerd's and the no-mailer node's).
  Against the base branch run alike (324 passed on celld), every check is
  the same, word for word, and passes; the one new is the restart's
  sign-in, on either provider.
- **Dex v2.45.1** (its image, `--network host`, memory storage, one static
  user, signing keys rotated every 20 s) on the dev stack on celld
  (`FRAGMENT_DEV_PORT=9310`, `FRAGMENT_OIDC_ISSUER=http://127.0.0.1:5556/dex`,
  the client's secret from a file); a script drove the browser's part.
  `/auth/login` went to Dex with S256, a state and a nonce; the password
  form posted, Dex sent the browser back, and the callback set the session:
  a new person, `npub1n7c5…kpmd4`, their sign-in `(http://127.0.0.1:5556/dex,
  <Dex's sub>)`, `admin@example.com` their email (Dex verifies its local
  users'). Signing in again was the same person; a wrong password signed no
  one in. `fragment login` printed a code, typed at `/cli` in that session
  ("act as admin@example.com"), and `fragment whoami` answered the person
  and the key. 18 sign-ins across 70 s of rotations all signed in, none
  refused. "Use another account" sent `prompt=login`, and Dex asked for
  the password. Logout was local only (Dex advertises no
  `end_session_endpoint`): 302 home, the cookie cleared, the session 401
  after. A refusal at the provider was a 400 page, and the node's log said
  `{"event":"signin.refused","issuer":"http://127.0.0.1:5556/dex","why":"the
  provider answered the authorization access_denied: …"}`.
- **The signature by WebCrypto** (2026-10-22, on `36d781dd`): `cargo
  xtask check` 838 passed, 0 failed (one new: the seam is handed the
  JWK, the input and the signature, is asked only after every rule before
  it, and its no or its failure refuses the token before any claim is
  read). The e2e's spoiled id_tokens grow to eleven: a forgery under a
  published kid (`Spoil::ForeignKey`) beside a byte changed, under RS256
  and ES256 alike, each refused as a signature that does not verify. The
  same sections through the OpenID Connect fake: on celld **331 passed, 0
  failed, 5 skipped**; on `wrangler dev` 327 passed, 0 failed, 9 skipped.
  Through the WorkOS fake: on celld 328 passed, 0 failed, 5 skipped; on
  `wrangler dev` 324 passed, 0 failed, 9 skipped.

**For Paul:** the schema step's number (it collides with any other
branch's registry step 4); whether a deployment moving from WorkOS to a
provider should keep its people (decision 45 refuses each email to the
new issuer); and the Wasm headroom (30 KB left).

### 5. Files: macrofiche, the code-store contract (done)

macrofiche (github.com/futurepaul/macrofiche, a sibling project as
sandcastle is) answers code.storage's API over git on disk (git itself,
jailed), as its `docs/contract.md` reads it from this repo. Where the git
lives is configuration:

| Where the git lives | dev (`cargo xtask dev`) | the e2e |
|---|---|---|
| the fake in the process (the default) | nothing set | `FRAGMENT_E2E_CODESTORE` unset |
| macrofiche, started with the stack | `MACROFICHE_BIN` | `FRAGMENT_E2E_CODESTORE=macrofiche` and `MACROFICHE_BIN` |
| a store already running | `CODESTORAGE_API_URL`, `CODESTORAGE_ORG`, `CODESTORAGE_PRIVATE_KEY_FILE` (the cell's own variables; the key by its file) | the same, with `FRAGMENT_E2E_CODESTORE=external` |

- **`devstack::codestore`** writes macrofiche's config file (its listener,
  its state, the org with its public key, `OrgKey::public_pem` of the key
  the cell signs with) and runs it: on :8792 in dev, its repos in
  `target/devstack/macrofiche`; in the run's scratch in the e2e. It dies
  with its parent. A cell remembers each fragment's repo, so a dev stack
  moved to another store takes `--clean`.
- **The e2e on a store outside it:** the fake is not the store, so
  `Suite::commit` and `deploy` move git through the platform's routes, as
  the hosted lane does, and reads past the cell take the store's own
  routes with tokens signed by the org key (`crates/e2e/src/store.rs`).
  No store registers the cell's per-fragment webhook: refresh and the poll
  move the pins. A section that needs the fake's levers declares
  `Need::StoreFake` and is a skip that says so: appfiles, ops, effects,
  blobs, brain, ai, ledger, site and sync, not yet ported.
- **The `codestore` section** probes the store against the contract with
  the cell's own parsers (`fragment_core::codestorage`): packs refused
  with nothing landed, a file's identity headers, listings, branches and
  merges, the merge preview and its restore steps, a repo's delete, the
  auth floor. It runs only on a store outside the run.
- **Parity, 2026-10-22:** of what master calls (cell/src/cs.rs, the CLI's
  codestorage.rs), macrofiche lacked one route: `DELETE /api/repos/{repo}`,
  which a wipe calls. It answered `404 no_route`, which the cell reads as
  "never there", so a wipe finished and left the person's repo behind.
  Fixed in macrofiche#2 (open for Paul). The merge preview and its restore
  steps already matched. A pack that changes nothing still makes an empty
  commit on macrofiche's `main` (found 33): macrofiche#1, still open. The
  runs below used a binary built from `main` with both merged; against
  `main` alone, `codestore` fails 21 of its 38 checks and `wipe` its
  repo-deleted check.

```sh
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=../celld/target/release/celld \
  FRAGMENT_E2E_CODESTORE=macrofiche MACROFICHE_BIN=../macrofiche/target/release/macrofiche \
  cargo xtask e2e --only create,files,deploy,codestore,templates
MACROFICHE_BIN=../macrofiche/target/release/macrofiche cargo xtask dev --runtime celld --clean
```

**Evidence, 2026-10-22** (macrofiche built from `main` with #1 and #2):

- On celld, OpenBao and the renderer on: create, files, deploy, codestore
  (38 probes) and templates, **185 passed, 0 failed, 0 skipped**.
- On the fake under `wrangler dev`, the same sections: 147 passed, 0
  failed, 1 skipped (`codestore`, which probes only a store outside the
  run). `wipe` on `wrangler dev` with macrofiche: 46 passed, 0 failed, 1
  skipped (a fake's lever). `external` against a macrofiche started by
  hand (create, files, codestore): 99 passed, 0 failed.
- `cargo xtask dev --runtime celld` with `MACROFICHE_BIN`: ready in 3.6 s,
  stopped with every port free.

### 7. Preview cards: the renderer over chrome-headless-shell (done)

celld refuses the `browser` binding, so the cell also takes its browser
from configuration: `FRAGMENT_BROWSER_URL` names a service answering the
binding's three routes (`POST /v1/devtools/browser`, the CDP socket at
`/v1/devtools/browser/<id>`, `DELETE`), reached with `fetch`; it wins over
the binding. card.rs keeps master's one path (the fragment's alarm takes
the shot: acquire, CDP with the share link set as a cookie and the page's
errors heard, release); only where its fetch goes differs (`Renderer`).
With neither, the shot is skipped (`card.skipped`, `no_browser`).

- **The renderer** (`crates/devstack/src/rendering`), the spike's: a
  browser per session over `--remote-debugging-pipe` (no port), at most
  four, each with a fresh profile and home and its own process group,
  Chrome's sandbox on, stopped after its `keep_alive` without a client.
- **Isolation:** every connection a card's page makes goes through the
  gate, a SOCKS5 proxy (with `<-loopback>`, so loopback and `*.localhost`
  too) that lets `<one label with "--">.<suffix>:<port>` through to one
  address and resolves no name; the browser resolves nothing itself, and
  sends no QUIC and no unproxied WebRTC UDP. The engine, a model server,
  the sign-in provider and the LAN are out of its reach.
- **The pin:** chrome-headless-shell **156.0.8078.47**, Chrome for
  Testing's Stable on 2026-10-22 (the spike's was 154). Chrome for Testing
  publishes no checksums: each platform's zip (linux64, linux-arm64,
  mac-arm64, mac-x64) was fetched, its MD5 matched against the
  `x-goog-hash` Google's bucket answers, and its SHA-256 pinned
  (`browser_release.rs`). `FRAGMENT_BROWSER_ZIP` names an intranet's copy,
  checked against the same pin.
- **A LAN:** `FRAGMENT_BROWSER_CA_FILE` trusts a private CA (an NSS
  database made with `certutil` in each browser's home);
  `FRAGMENT_BROWSER_UPSTREAM` (`ip:port`) moves where the gate connects.
  The LAN mode (seam 9) sets both to its door's.
- The e2e on celld and `cargo xtask dev --runtime celld` start it; every
  card check runs on celld (`shoots_cards` and its skips are gone). Shots
  there are metered at Browser Rendering's price: seam 10's question.

- **Found 2026-10-22:** a killed browser's page stayed in a fragment's
  presence on celld: the todo template's page said "you and 1 other
  here", the card's browser the other, long after the renderer had killed
  it. celld seems to keep a socket whose client dropped its connection
  without a close (not looked into further; it would hold any visitor
  whose network drops: one for the upstream issues). The renderer now
  asks a session's browser to close (`Browser.close`, 2 s at most) before
  it kills its group, so its pages' sockets end with a close.

**Evidence, 2026-10-22:**

- On celld, OpenBao on: auth, signin, keys, secrets, ai, finite-private,
  connections, files, deploy, site, ledger and restart, **586 passed, 0
  failed, 3 skipped** (found 31's lost 413, and two that need a computer,
  seam 2). Every card check ran: a deploy's card and a viewer's copy, a
  second deploy's, revalidation by its tag, two failed shots then the
  card, five then one `card.failed`, a guest's and a members-only
  fragment's none, the shot's browser time on the owner's ledger. The
  templates section on macrofiche with cards: above (seam 5).
- `wrangler dev`, master's path (Browser Rendering's local mode): secrets
  and site, 108 passed, 0 failed; site, files, deploy and ledger, 241
  passed, 0 failed.
- Host tests: the renderer against stand-in browsers (sessions, CDP both
  ways, its refusals, idle and crashed sessions, the WebSocket framing),
  the gate, the pinned zip; and the two run by name against
  chrome-headless-shell 156 (`cargo test -p fragment-devstack rendering --
  --ignored`): a card's page reaching nothing but its own origin, and a
  private CA refused unnamed, trusted named.
- The cell's Wasm on this branch (seam 4 included): 6,471,209 bytes,
  28,791 under the budget; the renderer's switch is about 2.4 KB of it.

### 9. Hosts and TLS: the LAN mode (done)

Each fragment's own origin (`<label>--<suffix>.<zone>`) is the isolation
model, so every way of running needs a wildcard name and a matching
certificate, or a single-box exemption: `*.fragment.localhost` on one box
(a secure context with no TLS); on a home network or an intranet, a
delegated subzone under a CA the devices trust. An intranet's zone is
same-site with the company's other apps, so isolation rests on the origin
(host-only `__Host-` cookies, Origin checks), never on SameSite.

`cargo xtask dev --lan` (xtask/src/lan.rs, crates/lan; the guide is
docs/self-host-lan.md) serves the stack on celld to the home network as an
intranet runs it, every part the stack's child:

- **`fragment-lan serve`**, the one process holding 53, 80 and 443 (so the
  one binary given `cap_net_bind_service`): DNS for `fragment.home.arpa`
  (RFC 8375), each name in it the box's address, every other name
  forwarded to the router; the TLS front door, `dex.<zone>` to Dex and
  every other name to celld on loopback, with the forwarded headers its
  own and WebSockets spliced; the root's page over http. The CA is made
  once, its root **name-constrained to the zone**; the zone's wildcard is
  issued at each start.
- **Sign-in**: Dex (v2.46.1, pinned by its image layer's digest and its
  binary's SHA-256; its config refuses unknown fields since 2.46) through
  seam 4's OpenID Connect, one static user per person, passwords in files.
  No WorkOS, Stripe or mail fake: a self-hosted deployment has none, so an
  invite is not mailed, and the share sheet says so and shows the address
  to send (with no mailer, `mail::mailer`).
- **Secrets** in OpenBao (seam 12), its state beside the LAN's. **Cards**
  through the door, under the root (seam 7). celld trusts the root for the
  cell's own fetches (`devstack::celld::start`'s CA file,
  `CELLD_EXTRA_CA_FILE`).
- **Computers** on this machine's real engine: the stack runs the box's
  node (`SANDCASTLE_NODE_BIN`), its platform the door's https origin and
  its `ca_file` the root, in a process group of its own so it outlives the
  cell's drain at Ctrl-C (a node gone first cuts each awake computer's
  `wait`, which the cell reads as an exit). Its list names Hermes and the
  stub by the digests `cargo xtask node-images` loaded (the record it
  writes, `target/devstack/node-images.json`), each by architecture: a
  paired Mac's are its own record, kept in the LAN's state. New computers
  run Hermes, as master's default is (docs/computers.md, Stable and
  canary: a deployment's `hermes` by digest; on a node, a load's digest,
  and a rebuild is a new one, so a computer is told an update is ready).
  `FRAGMENT_NODES_FILE` replaces the list.
- **An agent opens the apps it makes** (found 40, done 2026-10-22): the
  engine refuses a computer every private address, and the platform's
  hosts are the box's, so an agent built and deployed an app, then spent
  its turn failing to open it. Each computer now intercepts the
  platform's own hosts, its origin and `*.<zone>`, on the platform's
  scheme and port, as it does `api.fragment.internal`, and the platform
  answers them itself (`egress_platform`; docs/computers.md, "The
  platform's hosts"). It is master's behaviour, not the LAN's: on
  Cloudflare the same requests go through the platform rather than the
  internet. **Whose request it is:** exactly what the guest put on it, as
  a browser's anywhere: an app shared by link opens with its share link,
  one for its members alone opens to no one, its owner a member and all.
  The intercept adds no one: not the owner (whose session a computer
  never holds), not an agent (whose requests are `api.fragment.internal`'s,
  signed as itself). It sets only what an edge sets: the scheme and port
  dialed, and `cf-connecting-ip` as the computer's id, which a limit
  counts by. A name under the zone that is no host of this deployment's
  (Dex's) is the router's 404: nothing past the platform. Hermes' browser
  trusts the interception CA by Chrome's managed policy
  (`CACertificates`): Chrome for Testing, Hermes' pinned Chromium, reads no
  system bundle, so every intercepted HTTPS page was an interstitial in
  it, on Cloudflare too. Node takes it from the system bundle with
  `NODE_USE_SYSTEM_CA=1` (its own roots refused it).
- **On a `.localhost` stack too** (found 40's other half, done
  2026-10-23): Paul's Hermes built an app on his local stack and could
  not open it in its own browser. Chrome and curl take a `.localhost`
  name for the machine's own loopback without asking DNS (RFC 6761), so
  `http://<app>.fragment.localhost:8790/` went to the guest's own
  127.0.0.1 and never left the VM. The egress could not have caught it
  anyway: sandcastle's proxy knows a connection's name only by the fake
  address its DNS gave for it (198.18.0.0/15, `fakeip.rs`), never by
  `Host` or SNI, and an HTTP intercept matches a name on port 80 alone
  (`rules.rs`, as Cloudflare's), so the platform's port missed it even by
  DNS. Now the bridge, which every image runs, listens on the guest's
  loopback at the platform's port when the computer's origin is plain
  HTTP under `.localhost` (images/bridge `loopback.rs`): it reads each
  connection's `Host`, dials a name in the zone on port 80 through the
  guest's resolver (with the internet off too: an intercepted name always
  resolves), and passes the bytes both ways, a WebSocket's included; any
  other host there is a 421. `egress_platform` now hands the router the
  platform's port on HTTP as on HTTPS, so the app's origin is the one its
  page names and its live socket's `Origin` passes (it handed it port 80:
  with the forwarder alone, Hermes' browser loaded the app and its list,
  over HTTP, and sat on "connecting…", its socket refused). The
  forwarder holds the platform's port on the guest's loopback (8790 on
  dev), so an agent's own server cannot take that port there. A client
  that asks DNS instead is the next bullet's.
- **A client that asks DNS, on a `.localhost` stack** (found 40's last
  part, done 2026-10-23, as Paul chose: sandcastle's intercepts, not
  `libnss-myhostname` in the Hermes image). glibc asks DNS for a
  `.localhost` name (so Python and Node do), and the engine answers an
  intercepted name with an address of the egress's own; the client dials
  it on the platform's port (8790 on dev), never the guest's loopback, so
  the forwarder never sees it, and an HTTP intercept took a name on 80
  alone (as Cloudflare's do): the connection went on to the name's real
  host, which is no public address, and failed. Now sandcastle's HTTP
  intercepts take a name with a port (`name:port`, `*.glob:port`;
  sandcastle `node-http-port`, its docs/node.md, Intercepts), beyond
  Cloudflare's, and on a node the cell catches the platform's hosts on the
  platform's port as well as on 80, where the forwarder dials
  (`platform_intercepts`' `on_node`; docs/computers.md, "The platform's
  hosts"). Cloudflare's path is unchanged: a deployment without nodes is
  sent the same intercepts as before. `egress_platform` gives the router
  the platform's scheme and port whichever port caught it. **The engine
  must come first:** an engine older than that branch refuses the port's
  target (400, "not a host, address, ip:port, or range"), and with it
  every start on a `.localhost` stack's nodes; the Docker double takes it
  (a glob on another HTTP port, unresolved, as on 80).
- **With the internet off** (Paul's lockdown, 2026-10-22: later, not
  built): the platform's hosts reach the platform and nothing past it, so
  the design holds with `enableInternet: false`. A test fleet's lever,
  `offline {on}`, starts a computer so (nothing else does); the
  computers lane then opens its owner's app through the platform, reads
  its view through the API, calls a model through the route, and reaches
  no public host. The flag already ran end to end (the cell's start, the
  node, the engine's `Policy.internet`); the cell passed `true` to every
  start. What a mode needs past this: a setting that starts computers so,
  in place of the lever; and whatever Hermes fetches from the internet
  (skills from npm or a CDN, its installs, the CLI's updates) then fails
  unless the platform serves it.
- **The state** is `~/.local/state/fragment-lan` by default, outside the
  checkout: the CA, Dex's secrets, OpenBao's, the box's node's, and the
  deployment's models (`models.jsonc`, `model.key`), which dev reads when
  `FRAGMENT_MODELS_FILE` names none.

**Evidence, 2026-10-22:** the guide's (docs/self-host-lan.md, Evidence):
from a fresh worktree of the tip, every step on this box short of sudo,
the router and the devices, the iPhone a phone-sized headless Chromium.
Sign-in through Dex in 2.5 s; Hermes' first answer 9 s after the shell
landed (Bonsai, on a microVM of the real engine); a command turn in 4.8 s;
an app asked of the agent built and deployed in 90 s, its card shot
through the door; a live change in a second tab in 231 ms; `fragment
login` through the door; a second person's Mac (the Docker double, paired
through the door) holding their first computer; Ctrl-C in 0.2 s, every
port freed, the awake computer found as it was at the next start. Host
tests: crates/lan's 27 (the zone's DNS answers and forwarding, the
certificates and the name constraint, the door's routing, headers,
streams and WebSockets, Dex's config and the pinned layer) and xtask's
LAN settings and node list.

After it, on the branch's tip (celld v0.6.3, OpenBao and the renderer on):

- **`cargo xtask check`:** 835 passed, 0 failed.
- **The real engine** (`FRAGMENT_E2E_NODES=real`; computers, chat,
  hermes): **428 passed, 0 failed, 1 skipped** (the stub's awake save,
  master's own skip), as before seam 9.
- **Two nodes** (`FRAGMENT_E2E_NODES=two`; placement, pairing, restart):
  **112 passed, 0 failed, 0 skipped**, the no-mailer node's new check of
  the share sheet among them.
- The cell's Wasm: 6,698,165 bytes, 1,835 under `CELL_WASM_MAX_BYTES`
  (the sheet's no-mail words, 934 bytes).

**Evidence for found 40, 2026-10-22** (branch `selfhost-f40`, on
`selfhost` 84d8766e; master's half is three draft PRs, each a decision
for Paul: #533 the image's CA trust, #534 the platform's hosts, #535 a
local fleet's model names):

- **`cargo xtask check`:** 839 passed, 0 failed (its schema lock held
  to d8e0f86b, the master the lane is ported onto: `origin/master` has
  since released a Registry step the lane lacks, which the next port
  takes).
- **wrangler, local Docker** (computers): 204 passed, 0 failed, 2
  skipped (the hosted lane's), the platform's hosts' 4 and the lockdown's
  4 among them. The stub opened the app with its share link (200),
  without it and made members only it got 401, a name no one has 404;
  with the internet off it still opened the app, read its view, called a
  model, and `example.com` failed.
- **Two nodes on celld** (the Docker double; computers, chat, shell-ui,
  placement, pairing, restart): 616 passed, 2 failed, 9 skipped; the two
  were shell-ui stopping under a concurrent build's load, and shell-ui
  alone again passed 205/0. The double resolves no name under a glob nor
  turns the internet off, so those 8 checks are skips there (sandcastle
  `node-platform-globs`: the double takes a glob instead of failing every
  start).
- **Bonsai at `http://bonsai.localhost/v1`** from a Worker: 200 under
  celld and under wrangler's workerd.
- **The real engine and Hermes on the LAN:** not run yet (the engine was
  Paul's for a live test); the lane's run, and Hermes opening its own app
  with its browser and curl over the door's https, come next.

**Evidence for found 40 on `.localhost`, 2026-10-23** (branch
`selfhost-localhost-apps`, on `selfhost` a8e0baef; sandcastle `node`
57cd969 and celld unchanged; every run on celld):

- **Before** (the new checks alone, the bridge without its forwarder; the
  real engine, computers): 202 passed, **6 failed**, 1 skipped: each of the
  stub's opens of the app at its own address
  (`http://<app>.fragment.localhost:<port>/`, on its loopback as Chrome and
  curl dial it) was `client error (Connect)`, the lockdown's too.
- **The real engine** (`FRAGMENT_E2E_NODES=real`, computers): 208 passed,
  0 failed, 1 skipped (the stub's awake save, master's own skip): at its
  own address the app opened with its share link (200), without it and
  members only it did not, a name no one has was the router's 404,
  `localhost:<port>` the forwarder's 421, and with the internet off it
  opened.
- **The real engine, Hermes** (hermes): 71 passed, 0 failed: curl in
  Hermes' terminal read the app it made, and agent-browser's Chromium
  opened it and showed who is here, which comes over its live socket
  alone. With the forwarder but not the port: 70 passed, 1 failed,
  `browser-dead: connecting… Add oat milk ×` (its list over HTTP, its
  socket refused).
- **Two nodes on the Docker double** (computers): 199 passed, 0 failed, 10
  skipped (the double's 9 of the platform's hosts and the lockdown: it
  resolves no name under a glob).
- **wrangler, local Docker** (computers, master's path): 207 passed, 0
  failed, 2 skipped (the hosted lane's): the forwarder's dial on 80
  reaches workerd's egress too.
- **`cargo xtask check`** (`FRAGMENT_SCHEMA_BASE=d8e0f86b`): 848 passed, 0
  failed; `images/`: 262 passed, clippy clean.

**Evidence for a client that asks DNS, 2026-10-23** (branch
`selfhost-http-port`, on `selfhost` 07895358; sandcastle `node-http-port`,
on `node` 57cd969; celld unchanged; every run on celld):

- **Before** (the new check alone, the cell unchanged; the real engine,
  computers): 208 passed, **1 failed**, 1 skipped: busybox's `nslookup`
  got 198.18.0.3 for the app's name, the egress's own, and `wget` on the
  platform's port said "error getting response".
- **The real engine after: not run yet.** This box's engine
  (`~/.local/opt/sandcastle/bin`, Paul's) is older than
  `node-http-port`. Asked a start naming `*.fragment.localhost:8790` it
  answers 400, "not a host, address, ip:port, or range" (refused before
  anything starts). With the cell's change it refuses every computer's
  intercepts: the computers lane, stopped after 11 passed and 2 failed,
  had each wake end `start_failed`, "arm: the node box: PUT
  …/intercepts: 400 not a host, address, ip:port, or range:
  *.fragment.localhost:41715". The computers and hermes lanes on the
  real engine wait for the engine built from that branch.
- **Two nodes on the Docker double** (computers): 199 passed, 0 failed,
  11 skipped (the 10 before, and the new check: the double's DNS is
  Docker's). Every start there sent the port's glob, which the double
  takes.
- **wrangler, local Docker** (computers, master's path): 207 passed, 0
  failed, 3 skipped (the hosted lane's 2, and the new check: Cloudflare's
  containers take an HTTP host on 80 alone, and are sent the intercepts
  they were).
- **`cargo xtask check`** (`FRAGMENT_SCHEMA_BASE=d8e0f86b`): 848 passed, 0
  failed.
- **Sandcastle** (`node-http-port`): `cargo test`, 158 passed, 0 failed,
  2 ignored; among them the proxy's: a name looked up and dialed on its
  port reaches the handler, on 80 the intercept on 80, on another port
  nothing. The double's Docker test (ignored by default, run with the
  stub) passes, its intercepts check brought up to the double's globs.
  Clippy clean.

### 10. Money: who used what, in units (done)

Paul, 2026-10-22: "credits don't matter for the local version, just a
lightweight way for the admin to see who is using resources (on a shared
local inference setup for instance)". Credits and billing stay as master
has them (off with no Stripe key); what this seam adds is one view, in
units, of what master's ledger already records.

- **Read from the ledger; nothing new kept.** Every model call is a meter
  row on its payer's ledger (`aig:…`, after `model.settled`), its model
  and tokens in it, whatever it charged: a self-hosted model, priced at
  zero through its slot's ceiling (seam 3), is kept at a charge of 0, as
  is a job's AI step on it. Master drops no row for costing nothing (the
  ledger keeps each row it prices; the host test below meters Bonsai's
  calls through `Ledger::meter` and finds them), so nothing went to
  master. A ledger keeps its rows 90 days, so this month is whole.
  `fragment_core::ledger::used` is one query over a ledger's `entries`,
  host-tested on a real SQLite: this month's token rows by model (calls;
  tokens in, the cached among them; tokens out), the most calls first, and
  its awake rows by instance.
- **Own hardware is an instance priced at nothing.** A computer on a node
  its owner paired meters its awake time to their ledger as any computer
  does, on the instance `own-hardware` (`price::OWN_HARDWARE`), which the
  default book prices at zero: never charged, its hours theirs, apart from
  hours on the deployment's machines. Nothing on the call path: the meter
  is the computer's flush of its awake intervals, as before.
- **Where:**
  - `GET /api/ledger/usage`, a person's own (an agent's: its owner's):
    `Used {month, models: [{model, calls, inputTokens, outputTokens}],
    awakeMs, ownAwakeMs}`; Settings shows it as "Usage this month".
  - `GET /api/admin/usage?q=&after=`, operators': the people a page of 100
    at a time as `/api/admin/people` pages them, each one's ledger asked
    at once, `{people: [{npub, email, used}], next}` (anyone else: 403).
    The admin page's Usage tab is its one table (person; a model, or their
    computer on the deployment's machines or their own; calls; tokens in
    and out; hours), and `fragment operator usage [--q] [--after]` prints
    it.
- **Points are cut** (a decision for Paul): hours shown directly made
  them a second unit for the same use, kept by a second mechanism (the
  computer's own `own_hardware` meta row, `price::points`,
  `ownHardwarePoints` in its view, Settings' points line). The ledger
  already had the rows; an hour on one's own machine is now one of them.
  Lost: own hardware's use in list-price terms, which is its hours times
  the default instance's $0.064224.
- **Cut from the spike, a decision for Paul (as before):** a wake on own
  hardware was let past a person's want of credit. It is refused at zero
  credit as every agent's start is (decision 27): their agents' model
  calls are the platform's and stop then anyway, and the pass-over needed
  the wake to place the computer first.
- A computer on one of the deployment's nodes is billed exactly as before.
- **Evidence, 2026-10-22:**
  - `cargo xtask check`: 837 passed, 0 failed. Among the host tests,
    `ledger::used`: a seat's ledger meters a self-hosted deployment's
    calls (Bonsai's slots, priced from their ceiling of zero) beside a
    priced model, hours on the deployment's instance and on its owner's
    own, and other meters' rows, through `Ledger::meter`; the free rows are
    kept at a charge of 0, the balance moves by the priced ones alone, and
    the cell's own query over a real SQLite (the ledger's baseline step)
    reads this month's alone, last month's apart.
  - The e2e on `wrangler dev`, master's path (ledger, admin, admin-page,
    computers): 335 passed, 0 failed, 2 skipped (the hosted lane's). The
    ledger section ends on a node shaped as a self-hosted deployment
    (`Shape::SelfHosted`: every tier on a model of its own at no price):
    an agent's calls, streamed and not, are kept with their tokens at no
    charge, the balance as it was; the person, their agent, an operator's
    table and `fragment operator usage` read the same line; anyone else
    gets 403. The computers section reads its owner's hours on the
    deployment's machines in the operators' table; the admin page's Usage
    tab loads.
  - On celld (ledger, admin, admin-page): 135 passed, 0 failed, 3
    skipped (no computers without nodes).
  - Two nodes on celld (`FRAGMENT_E2E_NODES=two`; computers, pairing):
    245 passed, 0 failed, 1 skipped (the stub's awake save, master's own).
    A computer on its owner's node: its awake rows reach their ledger as
    own hardware at no charge, the balance unchanged, the operators' table
    and Settings show its hours as on their own machine, and they hold
    across a restart; one on the deployment's node is charged as ever,
    its hours the deployment's.
  - The cell's Wasm: 6,713,080 bytes, 6,920 under the 6.72 MB budget.

### 12. Secrets: the store's bindings, stood in for, OpenBao behind them (done)

The cell reads each deployment secret as `await env.<BINDING>.get()`
(cell/src/keys.rs, through workers-rs's `SecretStore`): the host secret,
the code.storage key, WorkOS's, Stripe's, the models' key, the operator
keys and connections' secrets. celld has no Secrets Store, so
`cell/secrets.mjs` gives the cell stand-ins with the same face: every
class in `cell/entry.mjs` takes its env through `withSecrets`, which, when
`FRAGMENT_SECRETS` names the deployment's secrets (the pairs a deploy
writes into `secrets_store_secrets`, and a backend), puts an object with an
async `get()` in each binding's place. Its class is named `Fetcher`, as
workerd's is, since workers-rs takes a store binding by that name (and
`keys::bound` relies on the check). Without `FRAGMENT_SECRETS`
(Cloudflare, `wrangler dev`) the env is as it came; the Rust is the same on
both and never knows which it read. Two backends:

- **`openbao`:** Vault's KV v2 API (OpenBao, or a company's own Vault):
  `GET <addr>/v1/<mount>/data/[<prefix>/]<secret_name>`, the value its
  field `value`, with the token in the variable `FRAGMENT_SECRETS_TOKEN`
  (and `X-Vault-Namespace` when `namespace` is set). The token is the one
  secret given outside the service, and the stand-ins hide it from the
  rest of the env. A read takes 5 s at most and follows no redirect; a
  failure is typed, never a hang: `SecretsError
  [missing|refused|sealed|down|failed|malformed]`, which keys.rs returns
  as `host_failed`. The shim caches nothing: keys.rs keeps a value a
  minute per isolate, as it keeps the store's, and never a failure.
- **`vars`:** the Worker variable of the binding's own name, which
  devstack writes into `.dev.vars` (`devstack::Secrets::Shim`).

**OpenBao** (crates/devstack/src/openbao.rs; Paul approved it 2026-10-05),
used as it ships:

- **The pin:** 2.7.1, still the newest 2.x on 2026-10-22: each platform's
  tarball by its SHA-256 in the release's `checksums.txt`, whose signature
  checked good against OpenBao's key (66D1 5FDD … 2853 E6D0) on 2026-10-05
  and again on 2026-10-22. Fetched once into `target/tools`;
  `FRAGMENT_OPENBAO_TARBALL` names an intranet's copy, checked the same.
- **Its state:** a directory of its own (0700): `seal.key`, 32 random
  bytes (0600), the static seal's, so every start unseals itself
  unattended; `data/`, pebbledb (2.7 removed `file`); the stack's AppRole
  ids (0600).
- **Initialised once, by itself:** the config's `initialize` stanza runs on
  the first start alone: KV v2 at `fragment/`, the two policies, the
  cell's token role and the stack's AppRole; then it revokes its root
  token. No root token or recovery key is left; a lost credential means
  removing the directory, and the next start seeds everything again.
- **Seeded at each start:** what `Fleet::configure` would write to
  wrangler's local store (`store::Bound`: master's Stripe and model keys
  among them) goes to `fragment/<its store name>`, a new version only
  where a value changed.
- **Secret zero:** the cell's token, policy `fragment-cell`: `read` on
  `fragment/data/*` and nothing else. Periodic (30 days), renewed at each
  start and hourly while `xtask dev` runs, minted again when it has
  lapsed; in `.dev.vars` (0600). The stack's AppRole writes the mount and
  mints the cell's tokens; it deletes nothing and reaches no `sys/` path.
- **Loopback only**, no TLS: the cell and OpenBao share the box.
- **Choosing it:** `cargo xtask dev --runtime celld` with
  `FRAGMENT_SECRETS_BACKEND=openbao` (on :8804, its state in
  `target/devstack/openbao-8804`, which `--clean` clears); the e2e on celld
  unless `FRAGMENT_E2E_SECRETS=vars`. The LAN mode (seam 9) makes
  it its default. `wrangler dev` keeps its own store.
- **A company's Vault:** `FRAGMENT_SECRETS` names it (`{"backend":
  "openbao", "addr": "https://vault.corp.example:8200", "mount": "secret",
  "prefix": "fragment", "namespace": "eng", "secrets": […]}`), each secret
  a KV v2 entry whose field is `value`, the cell's token in
  `FRAGMENT_SECRETS_TOKEN`, its CA in `CELLD_EXTRA_CA_FILE`. Only OpenBao
  is tested.

**Evidence, 2026-10-22:**

- Host tests (`openbao::tests`; the four on the binary run with
  `--include-ignored`): the state made once and refused spoiled; the
  config holding no secret; only the pinned tarball taken; the shim
  against a scripted server (each kind of failure, a hang cut at 5 s, no
  redirect followed, the token hidden) and a wrong config; and on OpenBao
  itself, initialised once and unsealed at every start, a wrong key said;
  the cell's token reading its mount alone; the shim reading a value and
  saying missing, refused, down and sealed. 8 passed, 0 failed.
- The e2e on celld, every secret in OpenBao: auth, signin, keys, secrets,
  ai, finite-private (the model key), connections (an OAuth client's
  secret), files, deploy, site, ledger and restart, **586 passed, 0
  failed, 3 skipped** (seam 7's run). restart stops OpenBao under a fresh
  node: a sign-in fails at once, naming it; started again, it unseals
  itself, initialised once, and the next sign-in works with no restart of
  the cell. The cell read the secrets 68 times (261 requests, 723 KiB of
  audit log): about once a binding an isolate.
- billing (Stripe's keys) on celld: its secrets read, but its four wipe
  checks fail, OpenBao or not: a wipe's computer step calls
  `ctx.container`, which celld lacks (`destroy: Cannot read properties of
  undefined (reading 'running')`): seam 2's.
- `cargo xtask dev --runtime celld` with `FRAGMENT_SECRETS_BACKEND=openbao`,
  started twice (a clean state, then the same): sign-in read WorkOS's
  client id through OpenBao, the cell's token 0600, and Ctrl-C freed
  every port.

## Where the time goes, 2026-10-22

Measured, not guessed: the same operations on master's path (`wrangler
dev`, its containers in local Docker), on celld with computers on two
nodes over sandcastle's Docker double (one that listens, one that dials
in), and on celld with one node in front of this box's real engine
(jailed microVMs, sandcastle `node` 57cd969; Paul's idle Hermes computer
stayed on the engine throughout, its one other VM). The bench is a lane on the branch `selfhost-perf-bench` (not for
merge: `cargo xtask e2e --only perf`, `FRAGMENT_PERF_SNAPSHOTS=1` for
snapshots), which logs each host call, each node call, each exec's
phases and each request's time in the router and in each object. Medians
of 20 runs (3 sleep, wake and restart cycles) on a quiet box (load 1 to
3), before the fixes below and after; the snapshot rows are from a run
under load (6 to 13: the box is shared, and one celld run under the
heaviest load read 50, 24 and 65 ms for the first three rows).

| Operation (ms) | wrangler, local Docker | celld, node that listens | celld, node that dials in | celld, the real engine |
|---|---|---|---|---|
| fragment op (`POST …/ops/say`) | 13 → 12 | 9 → 7 | (the same cell) | 9 |
| channel read | 9 | 5 → 4 | | 4 |
| live update (an op to its frame) | 13 → 11 | 9 → 8 | | 8 |
| a computer's view (`GET`) | 8–10 | 1 | 1 | 1 |
| cold start to ready | 570 | 1,310 → 335 | 1,160 → 240 | 130–145 |
| wake from its save | 665–680 | 1,490 → 525 | 1,440 → 495 | 245–310 |
| sleep (hold, save, stop) | 510–570 | 650 | 625 | 370–410 |
| restart, to awake | 5,590 → 1,270 | 5,510 → 1,175 | 5,470 → 1,135 | 620–780 |
| exec round trip (`true`) | 54–55 | 22 | 22 | 4 |
| guest → platform API | 11–14 | 1.4–1.6 | 1.4–1.5 | 1.2–1.4 |
| guest → model call (the fake) | 20–25 | 3 | 3 | 3 |
| snapshot save (loaded) | 190–300 | 220–470 | 120–450 | 58–80 |
| sleep with a snapshot (loaded) | 920–1,080 | 1,140–1,420 | 720–1,750 | 415–500 |
| wake from its snapshot (loaded) | 450–950 | 950–1,530 | 1,350–1,890 | 140 (360–420 loaded) |

The real engine's column comes from three runs after the fixes, two of
them with snapshots.
- On a quiet box, its starts were ready in 105 to 150 ms (`ready in`,
  the engine's journal), and a wake from a snapshot took 140 ms: the
  start and the hold's mark, no restore.
- While another session's e2e loaded the box (load 3 to 4), the same
  starts took 270 to 380 ms.

**Where it goes:**

- **The platform alone:** celld is no slower than workerd under
  `wrangler dev`. Each object answers within a millisecond on both.
  - The router's own time is 1 to 4 ms on celld, against 6 to 7 ms under
    wrangler (a computer's view: 1 against 7).
  - Wrangler's dev proxy adds 1 to 2 ms more.
  - An op is 9 ms in the router on either runtime, 3 to 5 of them in the
    fragment's object.
- **A start on a node, before:** `arm` took 1.4 s (median). That is the
  node's start, then 46 `PUT …/intercepts`, one for each host the swap
  catches, on http and on https, each sending the whole list again.
  - The node's start is 240 to 300 ms on the double (`docker run`, its
    relay, the relay's ready file polled every 25 ms).
  - Each PUT is 20 to 30 ms on the double (a `docker exec` rewriting
    `/etc/hosts`). After the fix, one PUT: `arm` is 290 ms.
- **A restart, before:** about 5 s of it was the stop's `EXIT_WAIT_MS`
  wait, watching the next container that the exit report had started
  (master's race), on every path.
- **An exec on a node:**
  - the WebSocket to the node: 1 ms;
  - the double's `docker exec`: about 22 ms;
  - the platform: under 1 ms;
  - the socket's close answered within 2 ms every time (never
    `CLOSE_WAIT_MS`).

  Under wrangler, workerd's local exec is about 53 ms.
- **The guest's calls on a node:** the node's egress, then
  `/api/nodes/egress` (1 to 2 ms in the router), the computer's object
  (HMAC, its binding), the route. Under wrangler, workerd's local egress
  is the slow hop (10 to 30 ms).
- **A wake from a snapshot on the double** waits 0.5 to 1.2 s for its
  guest: found 42. On the real engine it waits none (`heardInMs` 0).
- **Sleep:**
  - the hold's poll for `held` takes 150 ms (it polls every 100 ms);
  - the save takes 110 to 130 ms;
  - the stop's wait for the exit takes 200 to 300 ms (the stop polls
    `running` every 100 ms; on a node, `running` turns false when the
    node's `wait` answers).
- **A wake on the real engine** (245 to 310 ms from its save):
  - The node's start, answered once the guest is up: 113 ms (median),
    from the engine's own timings:
    - prepared at 0.7 ms, jailed at 5, the VMM built at 12;
    - the guest's hello at 109, its own uptime then 37 ms: about 60 ms
      go to libkrun loading the kernel before the guest's clock starts;
    - ready at 112.
  - Then:
    - the intercepts in one PUT: 1 ms;
    - the exec that finds it up: 8 ms;
    - the restore of `/data`: 56 ms;
    - the image's check, and the gate's mark: about 8 ms each.

  On the double, Docker's CLI is most of a wake:
  - its start (`docker run`, the relay) is 240 to 300 ms;
  - each exec is 22 ms (`docker exec`);
  - the restore is about 106 ms.

  On the engine, the VM's boot and the restore are most of it: a wake
  from a snapshot, with no restore, is 140 ms. A loaded box stretches
  the guest's boot: its uptime at hello went from 37 to 280 ms while
  another session's e2e ran.
- **Sleep on the real engine** (370 to 410 ms): two 100 ms polls are
  half of it: found 46.
  - The hold's poll for `held` takes 108 ms.
  - The stop's poll of `running` takes 102 ms.
  - The save takes 64 ms, the signal 8.
- **Execs and calls on the real engine:**
  - An exec is 4 ms in the object, its WebSocket to the node 0 to 1 ms.
  - An exec that prints four lines around a 50 ms sleep takes 52 ms,
    with no stall.
  - A guest's call to the platform is 1.2 to 1.4 ms, the same as on the
    double.

**Fixed:**

- **The restart race** (master #518, carried here): a restart is about
  4.3 s shorter on every path, and it answers `awake` (5.5 s to 1.2 s on
  the double, 5.6 s to 1.3 s under wrangler).
- **A start's intercepts in one call** (`NodeContainer.intercepting`):
  about 0.95 s off every start on the double on a quiet box, and 1.0 to
  1.9 s under load (a cold start, a wake from the save, a restart). On
  the real engine it saves the same 45 round trips through the node to
  its engine.

**Found:**

- **42. A wake from a snapshot on the double waits out the guest's
  backoff.** The real engine never does (`heardInMs` 0 at every wake
  from a snapshot): its start answers once the guest is up, and the
  start's one intercept PUT lands before the bridge's first read. The
  double answers once its relay is up, after its entrypoint started, so
  only the double's guest reads first. What follows is the double's
  case, a test stand-in's: no fix is needed for a real node.
  - The guest's first `GET /api/computer` comes before the start's
    intercepts are set (`arm` waits for the node's start, then sends
    them).
  - It fails at once, and the bridge waits its reconnect backoff
    (`RECONNECT_MS_MIN`, 1 s ± 50%).
  - The alive check (#362) waits for that read: 0.5 to 1.2 s of each
    wake from a snapshot on a node. Under wrangler it is 0: there, the
    intercepts are set before the guest's first request.
  - Fix, either way:
    - the node's start takes the intercepts (its API has them at start,
      and the list is the last arm's), with the bindings set as they are
      now;
    - or the bridge's first read retries every 100 ms for its first
      seconds. That is an image change, every deployment's: a decision
      for Paul.
- **43. Nagle on the node's listening sockets:** they lack
  `TCP_NODELAY` (the uplink and the egress set it), but nothing waits on
  it.
  - On the double, an exec with four lines of output is 77 ms either
    way.
  - On the real engine, the same exec is 52 ms around its 50 ms sleep,
    and a bare exec 4 ms.
  - sandcastle `node-perf` sets it, saves nothing, and can go.
- **44. The double is not the engine:** its exec, intercept and
  snapshot times are Docker's CLI (`docker exec`, `docker commit`).
  - A wake from the save takes twice as long on the double (495 to
    525 ms, against 245 to 310 on the engine).
  - A snapshot takes four times as long (120 to 470 ms, against 58 to
    80).
  - Read the engine's column for a node's time.
- **46. A sleep spends two 100 ms polls** (master's `ContainerHost`):
  - `execUntil` polls the guest's `held` every 100 ms;
  - `exited` polls `running` every 100 ms after the stop's SIGTERM.

  On the real engine that is about 210 ms of a 380 ms sleep, and of
  every restart; on Cloudflare too. A stop that awaits the container's
  exit (it watches it already, for the exit report), and a faster poll
  for `held`, would take most of it. A small master PR, if a sleep's
  200 ms is worth it: a decision for Paul.
- **45. celld's dev node halts when its lease store stalls.**
  - A lease renewal usually takes 2 to 4 ms. One took 2.5 s
    (`outcome="ambiguous"`).
  - The posture repair's `list` of `objects.sqlite3/nodes` then failed,
    and the node fenced itself (`SELF-FENCE … halting`, exit 3) at the
    lease's TTL. That ended an e2e run mid-section.
  - The box was loaded by other sessions at the time.
  - Its system lines are in `~/dev/finite/celld-issue-self-fence/`.
  - Likely found 34's store under load. An upstream celld issue for Paul,
    once it repeats on stock v0.6.3.
- Of master's lanes' waits on this branch, four stay, each for real
  latency:
  - the chat lane's push runs (celld's Workflows end later);
  - the hermes lane's ledger (found 38, celld's queues);
  - its one-second tool (found 24, Hermes' 0.3 s progress poll);
  - the always-on check's sleep under way.

  The restart waits went with the race.

## What the spike found, still true

The spike's numbering is kept; items gone with master's cuts (goose's
deadline, the agents' Worker) or fixed on master are left out.

6. **celld's `DurableObjectState` assigned `facets`**, so a redeploy that
   reused an isolate broke the app's next call. Fixed on the fork, which
   carries it on v0.6.3.
8. **`celld dev` stops on SIGINT**, not promptly on SIGTERM: devstack stops
   it so.
9. **A Worker's TCP Nagles**, and a Worker cannot stop it: the uplink's
   protocol carries that (seam 2).
17. **An object's answer was held after an outbound socket closed;**
18. **a socket one object returned to another closed with 1006.** Both
    fixed on the fork, which carries them on v0.6.3.
21. **celld ties a WebSocket to the request that opened it** (Paul's
    denoland/celld#254, unanswered): a computer's reports must come back in
    as requests of their own. Seam 2's.
22. **celld bounds an outbound fetch at 120 s** (`CELLD_FETCH_TIMEOUT_S`):
    a node's `wait` must outlive it (seam 2), and so must a model call
    slower than that.
26. **What a real model showed** (Bonsai under NInfer, Hermes): NInfer now
    answers `response_format: json_schema` (checked by curl, 2026-10-07);
    not yet seen in a live Hermes run.
31. **celld's early 413 to an upload it refused can be lost** (hyper
    closes on the unread body): a lingering close is an upstream celld
    issue for Paul to post.
33. **macrofiche committed a pack that changes nothing**: fixed in
    macrofiche#1, open for Paul (seam 5).
34. **`celld dev` reads its whole bucket for every listing:** long runs
    starve after crashes. The fork carries a range query as a stopgap on
    v0.6.3; an upstream issue for Paul.

New, 2026-10-22:

35. **celld's R2 and assets bindings' classes read as `Object`,** as its
    service bindings' do, so workers-rs's typed reads refuse them
    ("Binding cannot be cast to the type R2Bucket from Object"): every
    picture, blob and shell page failed. The cell reads `BLOBS` and
    `ASSETS` by what they do (`js::by_what_it_does`), as js.rs reads every
    other binding. An upstream celld issue for Paul, with the service
    bindings'.
36. **The model route takes http only at `127.0.0.1:<port>`**
    (`catalog::row_url`, which a test holds), so a model behind a local
    name or on a LAN's plain http (`http://bonsai.localhost/v1`,
    `http://192.168.50.7:8080/v1`) is refused at the config's parse. On a
    LAN, https under the operator's CA works (`CELLD_EXTRA_CA_FILE`).
    **Paul, 2026-10-22: yes** for this machine's names: a local fleet's
    model may now be http to a loopback address, `localhost` or a name
    under `.localhost`, on any port or none (seam 3); a LAN's other
    addresses stay https.
37. **A self-hosted model has no list price,** and master prices a model
    the book lacks only by its slot's `body.provider.max_price`,
    OpenRouter's routing field, sent to the server with each call. A price
    of a slot's own, not riding on OpenRouter's field, is seam 10's: a
    decision for Paul.
38. **celld's queues deliver a meter batch later than local workerd's**
    (past a second): the ledger's transcription checks counted the
    owner's rows before the last call's had landed. They now wait for it,
    on either runtime.
42. **A self-hosted model's long tool call read as a stall** (Paul's
    stack, 2026-10-22: a turn writing an app's files ended "custom
    rejected the request … Provider has been unresponsive (no response
    received) for 6 consecutive stale attempts"). NInfer streamed a tool
    call's arguments as one chunk at its end: asked to `write_file` a
    page, it streamed its reasoning, then nothing while it wrote the
    call, then all of it at once (60 lines, 5.1 s of silence; 300 lines,
    22 s and 12 KB, on an idle box at 351 tokens of context; the cell's
    log has calls of 45,000 tokens taking 45 to 105 s). Our Hermes image
    gave every text tier 20 s (`STALE_TIMEOUT_S`, tuned for cloud models'
    first bytes), and an explicit value overrides Hermes' own patience
    (180 s scaled by context, reasoning models' floors; its
    `_cloud_stale_timeout_for`). So Hermes killed each write at 20 s,
    tried it once more, then switched to the deployment's `fallback`: the
    same Bonsai, every try piling onto the one server. **Fixed** (master's
    too, as draft #527): a slot names its patience (`stale_timeout_s`,
    seam 3), which the guest's view carries to the image (`modelPatience`,
    beside `models`: inside it, the first port broke the computers lane's
    list of its ids); a fallback on
    the tier's own model is none.
    **Evidence** (a stack of its own on :8840, celld, a node over the
    Docker double, Bonsai through a pass-through that timed each call's
    chunks): Hermes, asked to write a garden planner of 350 lines or
    more in one `write_file`, wrote 840 lines in a turn of 176 s, its
    four calls 83 s (25 KB of arguments), 24 s, 57 s and 3 s. The third
    (34,528 tokens of context) streamed nothing for 56 s, behind another
    caller on the one GPU: 20 s would have killed it, tried it again,
    and switched to the same server. Its profile named
    `stale_timeout_seconds: 900` and no `fallback_providers`, and Hermes
    logged no stale kill. NInfer changed during the check (restarted at
    20:21 by its own session): it now streams a tool call's arguments as
    it writes them (1,739 and 3,322 chunks a call), so the silences left
    are a busy server's and a long prompt's. The hermes lane on celld
    with two nodes over the Docker double (`FRAGMENT_E2E_NODES=two`, by
    name): 60 passed, 0 failed. `cargo xtask check`: 838 passed, 0
    failed (held to master d8e0f86b's schema lock, the lane's base).
    **The next bound:** celld cuts an outbound fetch at 120 s in all
    (found 22), a model's stream included, however patient Hermes is: the
    first write took 83 s. A bigger write, or a busier box, ends there
    (`CELLD_FETCH_TIMEOUT_S`; a decision for Paul).

## Re-ported, 2026-10-22

Onto master d8e0f86b. celld: the fork's `selfhost` line on v0.6.3 (13
commits: its parity fixes, `CELLD_EXTRA_CA_FILE`, found 34's stopgap),
built 2026-10-22 15:20.

- **`cargo xtask check`:** 742 passed, 0 failed.
- **The e2e on celld** (auth, create, keys, members, signin, secrets,
  deploy, site, channels, live, jobs, blobs, ledger, restart): **745
  passed, 0 failed, 8 skipped**, each saying why: five a card's shot
  (seam 7), two a computer's (seam 2: the own key's sign-in and an
  agent's face across a restart), one found 31's lost 413. Among them the
  checks celld alone makes: a job sleeping through a crash wakes and
  finishes once; a deploy's card skipped, saying `no_browser`; and the
  node with no mailer (an invite to someone new, their first sign-in as
  it, two `mail.unsent` warnings in the log). On v0.6.2's fork binary
  (before found 35's fix and the gates): 683 passed, 2 failed, both since
  fixed.
- **The same sections on `wrangler dev`, master's path:** 758 passed, 0
  failed, 3 skipped (local Workflows' crash, and the no-mailer node's two:
  wrangler binds Email Sending).
- **An app Hermes made on Bonsai, opened in Chrome** (`agent-app`, on
  celld and the real engine, its tiers on `bonsai-2-27b`): asked for a
  shared grocery list, Hermes made one in one turn (506 s, 39 steps: it
  read the apps-finite skill, began from the todo template, rewrote its
  three files and deployed with the CLI), and Chrome opened it as its
  owner in two tabs: **13 passed, 0 failed**: it loads, throws nothing,
  its live socket says hello, each tab sees the other, an item added in
  one shows live in the other, and its `list` query answered every run.
  Its own browser never opened it: in a computer, Chrome resolves
  `*.localhost` to its own loopback without asking, so the intercept of
  the platform's hosts (found 40) never sees the request on a stack whose
  zone is `fragment.localhost` (it tried four addresses, then reported the
  app live from `fragment status`). The LAN's zone is a real name, and so
  is Cloudflare's. Since fixed (found 40, "On a `.localhost` stack too",
  2026-10-23): the bridge's loopback forwarder, and the hermes lane's
  check that Hermes opens its app with curl and in its browser.
- **Bonsai** (`FRAGMENT_MODELS_FILE`, its tiers and fallback on
  `bonsai-2-27b`): an agent's `POST /api/models/v1/chat/completions`
  (`model: cheap`) through the dev stack, 200 in 565 ms on celld ("Tomatoes
  'like' the sun because sunlight gives them the energy they need to
  photosynthesize and grow."; 51 tokens in, 67 out, metered as
  `aig:…` at zero) and 200 in 726 ms on `wrangler dev`. `cargo xtask dev`
  on either runtime started, answered, and stopped on Ctrl-C with every
  port free.

## Running it

```sh
# the stack on celld (CELLD_BIN: the fork's selfhost line, cargo build --release -p celld)
CELLD_BIN=../celld/target/release/celld cargo xtask dev --runtime celld
# its model on Bonsai: a deployment's `models` in a file, its key in another
FRAGMENT_MODELS_FILE=bonsai-models.jsonc FRAGMENT_MODEL_KEY_FILE=bonsai-key \
  CELLD_BIN=../celld/target/release/celld cargo xtask dev --runtime celld
# the e2e on celld
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=../celld/target/release/celld cargo xtask e2e --only auth,signin,…
# computers on two nodes over sandcastle's Docker double (SANDCASTLE_DIR: its `node`
# branch, sandcastle-node and sandcastle-docker-engine built, and sandcastle-docker-relay
# --target x86_64-unknown-linux-musl), one that listens and one that dials in
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… FRAGMENT_E2E_NODES=two SANDCASTLE_DIR=../sandcastle-node \
  cargo xtask e2e --only computers,chat,shell-ui,placement,pairing,restart
# one node in front of this machine's engine (SANDCASTLE_ENGINE_DIR, default
# /var/lib/sandcastle): each container its node starts carries the run's
# label (fragment-run, sandcastle's node `labels`), and the run destroys those
# alone at its end or on a signal; other nodes' containers there stay. The
# engine is sandcastle's node-http-port or later: an older one refuses the
# platform's port intercept on a .localhost stack, and so every start there
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… FRAGMENT_E2E_NODES=real SANDCASTLE_DIR=../sandcastle-node \
  cargo xtask e2e --only computers,chat,hermes
# an app an agent made, opened in Chrome (lanes/agent_app.rs): agent-app alone, its
# Hermes on the box's own model (a deployment's `models` in a file, its key in another;
# celld's fetch bound raised as dev's start.sh raises it, for a local model's long streams).
# On the real engine: Hermes intercepts `*.<zone>` (found 40), which the Docker double,
# routing exact hosts alone, refuses
FRAGMENT_E2E_MODELS_FILE=bonsai-models.jsonc FRAGMENT_E2E_MODEL_KEY_FILE=bonsai-key CELLD_FETCH_TIMEOUT_S=900 \
  FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… FRAGMENT_E2E_NODES=real SANDCASTLE_DIR=../sandcastle-node \
  cargo xtask e2e --only agent-app
# the stub and Hermes built and loaded into the engine, named by digest
cargo xtask node-images
# the stack with computers on nodes: a node list, each node's "secret_file" beside it
FRAGMENT_NODES_FILE=nodes.json [FRAGMENT_BYOC=on] CELLD_BIN=… cargo xtask dev --runtime celld
# the timings ("Where the time goes"), from the branch selfhost-perf-bench
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… FRAGMENT_E2E_NODES=two SANDCASTLE_DIR=… cargo xtask e2e --only perf
# the home network as an intranet (docs/self-host-lan.md, the guide)
CELLD_BIN=… SANDCASTLE_NODE_BIN=~/.local/opt/sandcastle/bin/sandcastle-node cargo xtask dev --lan
```

## Open questions for Paul

- **Found 37:** a self-hosted model's price from its slot (say `price`),
  rather than OpenRouter's `max_price` in its body.
- **Sign-in's sessions against the provider's** (seam 4): a platform
  session lasts 30 days whatever the provider says; back-channel logout,
  or a shorter session, when a company asks.
- **Upstream celld issues** to post: found 31 (a lingering close), 34 (a
  range query in `LocalStore`; the fork carries a stopgap), 35 (binding
  classes named as workerd's), and perhaps 45 (a dev node fencing itself
  when its lease store stalls). The timings found nothing else of celld's:
  its platform requests are no slower than workerd's under wrangler.
- **Found 39 (seam 2), fixed 2026-10-22 as Paul chose: a silent node is
  no exit.** It was: when a `wait` was cut and the node could not be asked
  whether the container still ran, `NodeContainer` threw, so any outage of
  a node (its process restarted, a blip, its uplink dropping) ended every
  awake computer on it, started again and rolled back to its last save,
  though the container ran on. Now the watch asks until the node answers,
  and past two minutes the computer says its node is down, typed, and goes
  on when it answers (seam 2, "A node silent under an awake computer"; the
  placement lane's `outage`). What remains, a decision for Paul: an idle
  computer whose node stays down still sleeps unsaved after its 20 idle
  minutes and the 30-minute unsaved bound, as any whose saves keep failing
  does. Stopping that clock while its node is silent would be a change to
  the lifecycle (crates/core), not to the watch.
- **Found 41: a phone shows no card**: the shell shows an app's card only
  over its dock icon under a pointer (master's shell).
- **Found 42's next bound:** celld cuts an outbound fetch at 120 s in
  all (`CELLD_FETCH_TIMEOUT_S`), so a model call that streams longer (a
  big write on a busy box) is cut whatever its patience. Raising it for
  the self-hosted stacks (dev and the LAN start celld) would bound a
  black-hole host later too.
- **The CLI's invite** still says "they were mailed a link" on a
  deployment with no mailer: telling it would add a field to `Shared`'s
  answer (add-only).
- **The cell's Wasm size depends on the checkout's path:**
  - 1,835 bytes left in `../fragment-port-lan`, and 1,583 in a checkout
    five characters longer;
  - on 2026-10-22, the same commit built to 6,700,438 bytes in one
    checkout and 6,709,219 in another;
  - remapping the path prefixes saved 1.6 KB of that 9 KB.

  The lane's budget is 6.72 MB.
- **Found 46 (above):** a sleep's two 100 ms polls (master's), about
  210 ms of every sleep and restart: filed as #552, not urgent.
