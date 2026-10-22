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
| 4 | Sign-in: OpenID Connect | **done** (below): beside WorkOS, on master's npub/email identity |
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
  of the provider's JWKS only, RS256 (the `rsa` crate, public-key
  operations only) or ES256 (`p256`); never `none`, an HMAC, an algorithm
  the provider does not list, a `crit` header, an access token, or a key
  the token names. Then `iss` exactly, `aud` (and `azp` when there are
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

**Costs.** The cell's Wasm grows 127 KB (6.34 MB to 6.47 MB): the
verification code and its JSON, and `num-bigint-dig` (27 KB) under `rsa`.
30 KB of `CELL_WASM_MAX_BYTES` is left.

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

**For Paul:** the schema step's number (it collides with any other
branch's registry step 4); whether a deployment moving from WorkOS to a
provider should keep its people (decision 45 refuses each email to the
new issuer); and the Wasm headroom (30 KB left).

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
