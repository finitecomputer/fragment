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
requirements come after.

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
| 2 | Computers on sandcastle nodes: placement, the uplink, bring your own computer | to port |
| 3 | Models | master's; dev's models file **done** |
| 4 | Sign-in: OpenID Connect | to port, rebuilt on master's npub/email identity (Paul, 2026-10-22: WorkOS stays its own path unless sharing is simpler) |
| 5 | Files: macrofiche, the code-store contract | to port |
| 6 | Blobs | nothing to port: celld's R2, over its bucket or `celld dev`'s disk |
| 7 | Preview cards: the renderer over chrome-headless-shell | to port |
| 8 | Notifications | design only: push is off offline |
| 9 | Hosts and TLS: the LAN mode (`--lan`) | to port |
| 10 | Money: own hardware in points | to port |
| 11 | Nothing calls home | design only |
| 12 | Secrets | the stand-in **done** (`vars`); OpenBao behind it to port |
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
  "cheap":    { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "bonsai-key", "model": "bonsai-2-27b",
                "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } } },
  "medium":   { … the same },
  "fallback": { … the same }
}
```

- **Its URL** is NInfer's own port, not Caddy's `http://bonsai.localhost/v1`:
  the route takes https on a host name, or on a fleet whose egress is local
  `http://127.0.0.1:<port>` (`catalog::row_url`), and refuses the rest
  (found 36).
- **Its price** is its slot's ceiling, zero: a self-hosted model has no
  list price, and master prices a model the book lacks only from
  `body.provider.max_price`, OpenRouter's field, which goes to the server
  with each call (NInfer ignores it; found 37).

**Evidence:** see "Re-ported, 2026-10-22".

### 12. Secrets: the store's bindings, stood in for (the stand-in done)

The cell reads each deployment secret as `await env.<BINDING>.get()`
(cell/src/keys.rs, through workers-rs's `SecretStore`). celld has no
Secrets Store, so `cell/secrets.mjs` gives the cell stand-ins with the same
face: every class in `cell/entry.mjs` takes its env through `withSecrets`,
which, when `FRAGMENT_SECRETS` names the deployment's secrets (the pairs a
deploy writes into `secrets_store_secrets`, and a backend), puts an object
with an async `get()` in each binding's place. Its class is named
`Fetcher`, as workerd's is, since workers-rs takes a store binding by that
name (and `keys::bound` relies on the check). Without `FRAGMENT_SECRETS`
(Cloudflare, `wrangler dev`) the env is as it came; the Rust is the same on
both and never knows which it read.

- **`vars`, the backend today:** the Worker variable of the binding's own
  name, which devstack writes into `.dev.vars` (`devstack::Secrets::Shim`).
- **`BACKENDS`** is the one place another slots in: OpenBao next (Vault's
  KV v2 API; Paul approved it 2026-10-05), its token the one secret outside
  it. The spike's (`crates/devstack/src/openbao.rs`, `selfhost`) is the
  reference.

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
    Whether `*.localhost` and a port-less loopback URL should pass is a
    decision for Paul.
37. **A self-hosted model has no list price,** and master prices a model
    the book lacks only by its slot's `body.provider.max_price`,
    OpenRouter's routing field, sent to the server with each call. A price
    of a slot's own, not riding on OpenRouter's field, is seam 10's: a
    decision for Paul.
38. **celld's queues deliver a meter batch later than local workerd's**
    (past a second): the ledger's transcription checks counted the
    owner's rows before the last call's had landed. They now wait for it,
    on either runtime.

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
```

## Open questions for Paul

- **Found 36:** should the route take `*.localhost` names and a port-less
  loopback URL, so `http://bonsai.localhost/v1` works as written?
- **Found 37:** a self-hosted model's price from its slot (say `price`),
  rather than OpenRouter's `max_price` in its body.
- **Sign-in's sessions against the provider's** (seam 4): a platform
  session lasts 30 days whatever the provider says; back-channel logout,
  or a shorter session, when a company asks.
- **Upstream celld issues** to post: found 31 (a lingering close), 34 (a
  range query in `LocalStore`; the fork carries a stopgap), and 35
  (binding classes named as workerd's).
