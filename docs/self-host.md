# fragment self-hosted

Status: **phase 1 of the catch-up, 2026-11-11**: master's cell runs on
celld on one Linux box. `cargo xtask dev --runtime celld` serves master's
shell, and the e2e's sections that need no computer pass on celld. The
branch is `selfhost-2`, re-ported by seam onto master 8bc07e62 (one commit
or a few per seam), never rebased. Self-host lands on master this time
(Paul, 2026-11-11): each seam is switched on by configuration, master's
wrangler path unchanged, and each seam's commits become a small PR of
their own. The old lane, `selfhost` (draft #133: 58 commits on master
d8e0f86b of 2026-10-06), is the reference implementation and is
superseded by this one. Where this disagrees with `docs/cloudflare-v1.md`,
that doc wins for master.

Seam 4, sign-in by OpenID Connect (below), is on `selfhost-2-oidc`, a
draft into `selfhost-2` (2026-11-11): Paul's "OIDC everywhere, with tsidp
as the tailnet default", since identity must not depend on the network
(a true intranet has no Tailscale).

## The rule: one cell, interfaces, placement as configuration

The cell is written against Cloudflare's runtime APIs (Durable Objects,
the Worker Loader and facets, Workflows, Queues, R2, `ctx.container`) and
protocols for what sits outside a runtime (OpenAI's chat completions, the
code-store contract, the DevTools Protocol, Web Push). Self-hosting
supplies other implementations of the same interfaces. The cell never
asks "am I self-hosted?": it reads what it has from its configuration and
its bindings. A mode flag is a design bug.

## The seams

| # | Seam | This phase |
|---|---|---|
| 1 | The runtime: celld | **done** (below) |
| 2 | Computers on sandcastle nodes | out of scope: no computers on celld here (below) |
| 3 | Models from a file | **done** (below) |
| 4 | Sign-in by OpenID Connect | **done** (below): tsidp the tailnet's default, an org's own IdP on an intranet |
| 9 | LAN mode and Dex | out of scope: a tailnet front door replaces them |
| 5 | Files: macrofiche, the code-store contract | out of scope: the code.storage fake |
| 6 | Blobs | nothing to port: celld's R2, over `celld dev`'s disk |
| 7 | Preview cards: a renderer over chrome-headless-shell | out of scope: a card is skipped, saying why |
| 10 | Money: who used what, own hardware in points | out of scope |
| 12 | Secrets | **done**: the stand-in, OpenBao 2.7.2 behind it (the default), a variables backend for local fleets alone |
| — | Mail | **done** as debt: said and carried on (docs/technical-debt-ledger.md, "No mail without Email Sending") |

## 1. The runtime: celld

celld (github.com/futurepaul/celld, its `selfhost` line: upstream's v0.6.3,
still its latest, and the fork's parity fixes) runs the cell from a config
rendered for it, `wrangler.celld.jsonc` beside the cell's own
(`crates/devstack/src/celld.rs`, `DROPPED`), less what celld has no part
in. One `devstack::Node` runs either runtime behind the node's front door
(front.rs): celld listens on a port of its own, which the front door
learns from celld's `ready` line, as it learns workerd's from wrangler's.
Stop (SIGINT), crash (the group SIGKILLed) and the e2e's signal handler
serve both.

Every binding and runtime API master's cell uses, against the fork's celld
(`selfhost`, b19f09d):

| Binding or API | On celld | Class | How the cell runs |
|---|---|---|---|
| `durable_objects`: Fragment, Principal, Registry, Ledger, SearchIndex, Vault | yes | works | |
| `durable_objects`: Computer | the class runs | off | no computers: `FRAGMENT_COMPUTER_IMAGE` unset, so a computer is refused, typed ("this deployment makes no computers"); its container host reads no container as none running, so a person's wipe still empties their computer's rows |
| `durable_objects`: Migrations, and R2 `STAGE` | yes | off | the finite.computer move: unconfigured (`FRAGMENT_MIGRATIONS_MOVER` unset), as on any deployment but finite.chat's |
| `migrations` (`new_sqlite_classes`, v1 to v4) | yes | works | |
| `durable_objects.code_update_strategy` | accepted, no effect | works | celld reads the bindings alone; `celld dev` deploys once (`--no-watch`), and the e2e's reload restarts the node |
| `worker_loaders` (`LOADER`) and facets (`ctx.facets`) | yes, but `facets.abort` fails for a brain's facet (found 51) | works, one defect | apps' facets, their databases capped past the cell's own (`CELLD_FACET_MAX_BYTES`), loaded code locked down (`CELLD_DYNAMIC_LOCKDOWN`); a brain's install of a newer release breaks its facet |
| `workflows` (`JOBS`) | yes, durable | works | a sleep is an alarm, so a job sleeping through a crash wakes (local workerd's does not) |
| `queues` (`DELIVERIES`, `METERS`, the dead letters) | yes | works | |
| An object's requests in flight | 64 at most by default | works, configured | devstack runs celld with 512 (found 50) |
| `r2_buckets` (`BLOBS`) | yes; its class reads as `Object` | works | read by what it does (`js::by_what_it_does`, found 35) |
| `assets` (`ASSETS`, `run_worker_first`, `html_handling`, `not_found_handling`) | yes; its class reads as `Object` | works | read by what it does (found 35) |
| `secrets_store_secrets` | refused | stood in | the cell's stand-ins (seam 12), OpenBao behind them; people's secrets stay in the Vault object |
| `ai` (`AI`: Workers AI) | refused | degraded, typed refusal | text tiers on an OpenAI-compatible API (seam 3); a slot left on Workers AI, image steps, transcription and embeddings fail typed ("this node has no AI binding"). Dev and the e2e keep the fake (`FRAGMENT_AI_URL`) |
| `send_email` (`EMAIL`) | refused | degraded, said | no mail: each send logs `mail.unsent` and answers no id, never an error (below) |
| `browser` (`BROWSER`) | refused | degraded, said | no cards: a live is skipped once, `card.skipped` `no_browser`; asking for a shot again is refused, typed (`host_failed`). Seam 7 brings a renderer |
| `images` (`IMAGES`) | refused | degraded, typed refusal | no icons drawn: "this node has no IMAGES binding" |
| `containers` | runs on Docker, but without `interceptOutboundHttp`/`interceptOutboundHttps` or `snapshotContainer` (krun engine only) | off | a computer needs its egress intercepts and its saves: dropped by the render; seam 2 places computers on sandcastle nodes |
| `ctx.exports` | yes (`DurableObjectState.exports`) | works | used only by a computer's container host |
| `interceptOutboundHttp`, `interceptOutboundHttps`, `snapshotContainer` | krun engine only | off | with computers (seam 2) |
| `connect()` (`cloudflare:sockets`) | yes | off | the XFTP relay is a computer's |
| `compatibility_flags`: `nodejs_compat` | partial (its `node:` modules) | works | the Sandbox SDK's imports load |
| `compatibility_flags`: `containers_pid_namespace` | accepted, no effect | off | with computers |
| `build` | refused | n/a | worker-build has run (`cargo xtask build`) |
| A streamed request body forwarded to an object | its length is not carried | works | the router forwards the length it checked (found 47) |
| `CF-Connecting-IP` | not set | degraded | every CLI code's ask counts as one address, `unnamed`, unless a front proxy sets the header (found 48) |
| Cache API, HTMLRewriter, cron triggers | | n/a | the cell uses none |

- **Dev:** `cargo xtask dev --runtime celld` (`CELLD_BIN`; esbuild from
  worker-build's cache or `CELLD_ESBUILD`): the same fakes and settings as
  on wrangler, no computers, the secrets behind the stand-ins.
- **The e2e:** `FRAGMENT_E2E_RUNTIME=celld`. Its rung (`needs::Rung::Celld`)
  has everything a local run has but the runtime's containers: a section
  that needs computers is a skip saying seam 2. Checks celld cannot make
  are skips saying why (a card's shot; found 31's 413); the check local
  workerd cannot make runs (a job sleeping through a crash). The build
  makes no computer images. The restart section ends on a node with no
  mailer (`Shape::NoMailer`), then stops and starts OpenBao under a node.
- **Master's path is unchanged:** nothing branches on the runtime. Two
  reads by what a binding does, a card skipped where there is no browser,
  mail that says it is not sent, and a streamed upload's length forwarded:
  on wrangler and Cloudflare the bindings are there and the length was
  carried already.

## 3. Models from a file

`cargo xtask dev` reads `FRAGMENT_MODELS_FILE`: a deployment's `models`
block alone, as its config writes it, checked as a deploy checks it
(`deploy::models_cell`) but for its URLs, which may be this machine's
(`http://127.0.0.1:<port>`, as a local fleet's egress reaches it). The key
its OpenAI-compatible slots name is the value in `FRAGMENT_MODEL_KEY_FILE`
(one; a server that takes none still gets a word). Slots it leaves out,
and image steps, stay on the Workers AI fake. It is `--real-agent`'s
models without our Hermes image, which celld cannot run. A self-hosted
model names its price in its slot (master's `price`, 2026-10-22: found 37
is master's now), zero for the box's own:

```jsonc
{
  "cheap":  { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "bonsai-key", "model": "bonsai-2-27b",
              "price": { "input": 0, "output": 0 } },
  "medium": { … the same }
}
```

## 4. Sign-in: OpenID Connect

Any OpenID Connect provider signs people in where a deployment names
one: on a tailnet, Tailscale's tsidp (the default, below); on an
intranet, the org's own IdP (Entra, Okta, Google Workspace, Keycloak,
Authentik). It takes WorkOS's place, chosen by configuration, and never
stands beside it as a fallback. WorkOS stays finite.chat's sign-in, a
separate implementation (Paul, October): no migration between them.
Without `FRAGMENT_OIDC_ISSUER`, nothing here runs and master's WorkOS
path is as it was.

**Who a person is** (decisions 45 to 50, unchanged):
- A sign-in is `(issuer URL, sub)`. The first makes a person, named by
  the npub the registry makes and seals for them (`person_for`, the same
  function as WorkOS's).
- The email the provider gives is their handle. It must be there, and
  either verified (`email_verified: true`) or vouched for by the
  deployment (`FRAGMENT_OIDC_EMAIL_VOUCHED=on`).
- An email names one person: another person's is refused (409), never
  merged. The latest sign-in's email is the one shown.
- Sessions, the CLI's `/cli` approval, invites met at a first sign-in,
  fragments' redemptions and frames, and logout are WorkOS's own code,
  unchanged.
- A person a provider signs in has no WorkOS user, so no Pipes
  connections (direct OAuth connections are a later seam).
- A WorkOS person's email is refused to a provider (409, decision 45):
  nothing moves people between them.

**Settings**: Worker variables, the secret a store binding (OpenBao on
self-host, seam 12).

| Setting | What |
|---|---|
| `FRAGMENT_OIDC_ISSUER` | the issuer URL exactly as its id_tokens' `iss` (https; http only on loopback). Its metadata is at `<issuer>/.well-known/openid-configuration` |
| `FRAGMENT_OIDC_CLIENT_ID` | the client |
| `OIDC_CLIENT_SECRET` | the client's secret, a store binding read only by keys.rs (devstack's store name: `fragment-oidc-client-secret`). Unbound: a public client, PKCE its only proof |
| `FRAGMENT_OIDC_SCOPES` | default `openid email profile`; `openid` required |
| `FRAGMENT_OIDC_EMAIL_CLAIM` | default `email`. Another claim (ADFS's or Entra's `upn`) only when vouched for: `email_verified` speaks for `email` alone |
| `FRAGMENT_OIDC_EMAIL_VOUCHED` | `on`: the provider's word for the email is enough, as an org's own directory's is (tsidp's; below). Never for a provider whose people choose their own addresses (a multi-tenant or social one) |
| `FRAGMENT_OIDC_AUTH` | `client_secret_basic`, `client_secret_post` or `none`; default, with a secret, the first of the two the provider lists |

At the provider, the client's redirect URI is `<platform>/auth/callback`.
A setting out of shape stops the isolate at its first request, naming it
(`oidc::Settings::parse`).

**The flow.** Three places:
- **`crates/core/src/oidc.rs`, the rules:** every decision, pure, with
  its tests.
- **`cell/oidc.mjs`, the shim:** what JavaScript does for free. It
  fetches the provider's documents (bounded, kept per isolate), sends the
  token request, `JSON.parse`s, chooses the key and verifies with
  WebCrypto. It answers flat values and decides nothing more, but it
  refuses any algorithm but RS256 and ES256 on its own. Its tests are
  `cell/oidc.test.mjs`.
- **`cell/src/oidc.rs`, the router.**

The steps:
- **Begin** (`/auth/login`, `/auth/switch`): the provider's metadata
  first, so a provider that cannot answer leaves no pending sign-in. Then
  the registry begins a sign-in, as WorkOS's: its state, bound to the
  browser by `fragment_login`.
- **The verifier and the nonce** are derived from the host secret and the
  state (`SignInSecrets`: HKDF, then an HMAC of the state for each).
  Nothing is stored, and only the platform can make the verifier. A
  sign-in that spans a host-secret rotation fails at the provider and
  starts again.
- **To the provider**: the browser goes to its `authorization_endpoint`
  with the code flow, the S256 challenge, the nonce and a `login_hint`. A
  switch adds `prompt=login`.
- **The callback**: bound to the browser that began it (`trip_bound`).
  keys.rs makes the token request (form-encoded, the verifier, the client
  authenticated as chosen, its secret read there), and the shim sends it
  (`redeem`). No redirect is followed, and the answer is read (at most
  64 KiB) for its id_token alone.
- **The id_token**, by the shim:
  - Its algorithm must be one of the provider's that the rules allow
    (RS256, ES256), and none but those two in any case: no `none`, no
    HMAC. No `crit`, and no access token.
  - The key is from the provider's JWKS, never one the token names. An
    RSA key is 2048 to 4096 bits, an EC key P-256. The shim imports a JWK
    made from the key's numbers alone.
  - The signature is checked by WebCrypto: the cell carries no RSA crate.
  - Only then are the claims read.

  Then by the rules (`redeemed`):
  - the algorithm again;
  - each claim's type;
  - `iss` exactly, `aud` (and `azp`);
  - `exp`, `iat` and `nbf` within two minutes, and an `iat` at most 15
    minutes old;
  - the nonce, and a `sub`.
- **Finishing**: the email rule, then the registry spends the sign-in in
  one statement and sets the session, through WorkOS's own exchange call
  (`calls::Exchange::verified`, made only from the callback's `Bound`) and
  its own finish.
- **Refusals**, on a page, with `signin.refused` and why in the node's log:
  - an id_token not to trust: 401;
  - an email absent or unverified: 403;
  - another person's email: 409;
  - a code the provider refuses (spent, expired): 400.
- **Caches** (the shim's, per isolate): the metadata and the JWKS, an
  hour. A key the JWKS lacks fetches it again at once (a rotation), then
  not again for such a key within 15 s. Every limit is
  `oidc::SHIM_LIMITS`, handed to the shim, never copied into it.
- **Logout** ends the platform session here alone. tsidp offers no
  `end_session_endpoint`; RP-initiated logout is not built.
- **A restart mid-sign-in** finishes, once: the state is the registry's,
  and the verifier and nonce are derived again.

**tsidp, the tailnet's default.** Tailscale's own OIDC server, which signs
in a device's person from their tailnet identity. Experimental, by its own
README.
- **The pin:** v0.0.18 (2026-10-21, commit 4047a40f), image
  `ghcr.io/tailscale/tsidp:v0.0.18`, index digest
  `sha256:3b39de93c1dd1a6bd6923b8871957ab938d049128035634b2b7204a900fb4661`.
- **Why not newer:** v0.0.19 and v0.0.20 (2026-11-11 and -12) add only the
  `offline_access` scope and a version bump, and are younger than the
  two-week dependency cooldown.

What it does, read from its source at v0.0.18 (not run here: there is no
tailnet on this box):
- **The issuer** is `https://<TS_HOSTNAME>.<tailnet>.ts.net`, with
  `:<port>` added off 443. It serves on Tailscale's HTTPS certificates
  (`ListenTLS`), so MagicDNS and HTTPS must be on in the tailnet.
- **Signing:** RS256 alone, one 2048-bit key with a `kid`, never rotated.
  Its discovery lists `client_secret_basic` and `client_secret_post`,
  PKCE `plain` and `S256`, and no `end_session_endpoint`.
- **PKCE:** with no `code_challenge_method`, it assumes `plain`.
  Fragment always sends `S256`.
- **A client secret is required** at `/token`: it takes no public client.
  Redirect URIs are matched exactly.
- **The id_token's claims:**
  - `sub`: `userid:<n>`, the Tailscale user's id, stable.
  - `aud`: the client id, as a string.
  - `nonce`: echoed.
  - `exp` is `iat` plus 5 minutes; `nbf` is `iat` less 5 minutes.
  - `email`, only with the `email` scope: the user's login name, except
    that a `@github` or `@passkey` login becomes `<login>.<tsidp's
    host>`, such as `alice@github.idp.tail1234.ts.net`. That is not a
    mailbox.
  - **No `email_verified`, ever.** So a deployment on tsidp sets
    `FRAGMENT_OIDC_EMAIL_VOUCHED=on`: the tailnet's control plane vouches
    for its login names, and nobody picks another's. Instead, an admin
    may add `"extraClaims": {"email_verified": true}` to a
    `tailscale.com/cap/tsidp` grant, for people whose login names are
    real addresses (claims it does not list may be added so).
- **It ignores `prompt` and `login_hint`:** whoever owns the device signs
  in, with no screen. "Use another account" signs the same person in
  again.
- **The browser must be on the tailnet:** `/authorize` names the device
  by WhoIs on its address, and refuses a tagged device (it has no
  person). The cell's own calls to `/token` and the JWKS need no WhoIs:
  the box's node reaching tsidp's is enough.
- **Restarts:** codes live in tsidp's memory, so a tsidp restart in the
  middle of a sign-in fails it, and the person starts again.

**Setting it up** (an org's IT person):
1. **The tailnet:** MagicDNS and HTTPS certificates on. Make an auth key
   with a tag for tsidp (`tag:tsidp`).
2. **The policy:** people's devices, and the fragment box, reach the
   tsidp node on tcp:443 (`tag:tsidp` in `tagOwners`). The admin UI is
   for the IT person alone. This policy is written from Tailscale's grant
   syntax and tsidp's README, not tried on a tailnet:
   ```hujson
   "grants": [
     { "src": ["autogroup:member"], "dst": ["tag:tsidp"], "ip": ["tcp:443"] },
     { "src": ["autogroup:admin"], "dst": ["tag:tsidp"],
       "app": { "tailscale.com/cap/tsidp": [{ "allow_admin_ui": true }] } }
   ]
   ```
3. **Run tsidp** beside fragment, its state on a volume:
   ```sh
   docker run -d --name tsidp --restart unless-stopped -v tsidp-data:/data \
     -e TAILSCALE_USE_WIP_CODE=1 -e TS_STATE_DIR=/data -e TS_HOSTNAME=idp \
     -e TS_AUTHKEY="$(cat tsidp-authkey)" \
     ghcr.io/tailscale/tsidp:v0.0.18@sha256:3b39de93c1dd1a6bd6923b8871957ab938d049128035634b2b7204a900fb4661
   ```
   Its first start can take minutes, while its certificate is issued.
4. **Register fragment** at `https://idp.<tailnet>.ts.net/` (the admin
   UI): a new client named `fragment`, its redirect URI
   `https://<platform>/auth/callback`. tsidp shows its id (32 hex) and its
   secret (64 hex) once.
5. **Configure fragment:**
   - `FRAGMENT_OIDC_ISSUER=https://idp.<tailnet>.ts.net`;
   - `FRAGMENT_OIDC_CLIENT_ID=<the id>`;
   - `FRAGMENT_OIDC_EMAIL_VOUCHED=on`;
   - the secret in OpenBao, under the store name bound as
     `OIDC_CLIENT_SECRET`.
   - The email claim stays `email`, and the scopes stay
     `openid email profile`.
6. **Reachability:** the platform's own fetches (the metadata, the keys,
   the token request: cell/oidc.mjs) must reach `idp.<tailnet>.ts.net`, a
   100.64/10 address, which celld's `CELLD_EGRESS_PUBLIC_ONLY` refuses.
   The issuer's host must be open to the platform, and **never to apps**.

   **TODO, once fragment#1239 and futurepaul/celld#7 land** (neither is in
   this branch's base, and nothing here writes `CELLD_EGRESS_ALLOW` yet):

   - **Where celld's allow list is written,** add the configured issuer's
     host as one exact name: `url::Url::parse(FRAGMENT_OIDC_ISSUER)`'s
     `host_str()`, no glob. That list is #1239's
     `fragment_core::egress::Allow::celld()`, set as the node's
     `CELLD_EGRESS_ALLOW` beside the other `CELLD_*` variables in
     `crates/devstack/src/celld.rs` `start()` (and in any self-host
     launcher). Pass the issuer's host into `start()`, or into the
     `Allow` it renders.
   - **Not in the cell's app egress check.** The issuer's host must never
     be added to `FRAGMENT_EGRESS_ALLOW` or `fragment_core::egress::Allow`'s
     entries. That list is what `egress::check` holds apps' `job.fetch`,
     push endpoints and subscriptions to (cell/src/jobs.rs, push.rs,
     subscriptions.rs).
   - **Why this keeps apps out:** apps have no network of their own
     (`globalOutbound: null`), and every fetch they ask for passes
     `egress::check` first, so a host open at the node is still closed to
     them.
   - **tsidp's address:** under tsnet, tsidp has a tailnet address of its
     own, not one of the box's, so celld#7's own-address rule does not
     refuse it.

   Not verified here: there is no tailnet on this box, and the celld this
   branch runs predates celld#7.

**Dev.** The fake:
- `FRAGMENT_SIGNIN=oidc cargo xtask dev --runtime celld` signs people in
  through the OpenID Connect fake on :8804, a strict provider (any email,
  verified).
- `FRAGMENT_SIGNIN=tsidp` runs it shaped as tsidp: RS256 alone, no
  `email_verified`, vouched for.
- The fake's subjects come from its people's emails, so a restart of dev
  hands each email the subject it had before.

A real provider: `FRAGMENT_OIDC_ISSUER`, `FRAGMENT_OIDC_CLIENT_ID`,
`FRAGMENT_OIDC_CLIENT_SECRET_FILE`, and `FRAGMENT_OIDC_EMAIL_VOUCHED=on`
or `FRAGMENT_OIDC_EMAIL_CLAIM` as needed. Its redirect URI is
`http://127.0.0.1:8790/auth/callback`. WorkOS's fake runs beside it for
Pipes alone.

**The e2e:** the `oidc` section starts its node shaped to sign in through
the provider fake, then shapes the fake as tsidp, then goes back to a
plain node. Its checks are in `crates/e2e/src/lanes/oidc.rs`; the core
rules' tests are in `oidc::tests`.

**Not built:**
- admins from a groups claim (operators stay `FRAGMENT_OPERATORS`);
- `private_key_jwt`, the userinfo endpoint, refresh tokens, RP-initiated
  and back-channel logout;
- an `oidc` block in `xtask deploy`'s config: a Cloudflare deploy signs
  people in through WorkOS alone.

The platform's session lasts 30 days whatever the provider says, so a
person disabled at the provider keeps theirs until it ends or they sign
out.

## 12. Secrets: the store's bindings, stood in for

The cell reads each deployment secret as `await env.<BINDING>.get()`
(cell/src/keys.rs, through workers-rs's `SecretStore`). celld has no
Secrets Store, so `cell/secrets.mjs` gives the cell stand-ins with the
same face: every class in `cell/entry.mjs` that runs the platform's code
takes its env through `withSecrets`, which, when the Worker variable
`FRAGMENT_SECRETS` names the deployment's secrets (the pairs a deploy
writes into `secrets_store_secrets`, and a backend), puts an object with
an async `get()` in each binding's place. Its class is named `Fetcher`, as
workerd's is, since workers-rs takes a store binding by that name. Without
`FRAGMENT_SECRETS` (Cloudflare, `wrangler dev`) the env is as it came; the
Rust is the same on both and never knows which it read. Two backends:

- **`openbao`:** Vault's KV v2 API (OpenBao, or a company's own Vault):
  `GET <addr>/v1/<mount>/data/[<prefix>/]<secret_name>`, the value its
  field `value`, with the token in `FRAGMENT_SECRETS_TOKEN` (and
  `X-Vault-Namespace` when `namespace` is set). The token is the one
  secret given outside the service, and the stand-ins hide it from the
  rest of the env. A read takes 5 s at most and follows no redirect; a
  failure is typed, never a hang (`SecretsError
  [missing|refused|sealed|down|failed|malformed]`). The shim caches
  nothing: keys.rs keeps a value a minute per isolate, as it keeps the
  store's.
- **`vars`:** the Worker variable of the binding's own name, in
  `.dev.vars` (`devstack::Secrets::Shim`): a convenience for dev and the
  e2e alone. The shim refuses it on any fleet that is not local
  (`FRAGMENT_EGRESS_LOCAL=allow`).

**OpenBao is a self-hosted deployment's backend** (Paul, 2026-11-11): it
stands in for Cloudflare's Secrets Store, holding the platform's root
secrets (the host secret, the code store's key, sign-in's and the
connections' client secrets, the model keys, the operator keys). A
`FRAGMENT_SECRETS` that names no backend is refused at the first request,
loudly; nothing falls back to the file. People's own secrets are not
here: they stay in master's Vault Durable Object, sealed under keys
derived from the host secret, as on Cloudflare.

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
  lapsed; in `.dev.vars` (0600).
- **Loopback only**, no TLS: the cell and OpenBao share the box.
- **Its audit log is on:** every request, its secrets HMAC'd by OpenBao,
  in `audit.log` beside its data (the e2e counts the cell's reads from it).
  A deployment rotates it as it rotates any log.
- **Its unseal key** is the static seal's key file, `seal.key` in its
  state directory (0600, beside its data): whoever reads that directory
  can unseal it. The hardening path, not built: a TPM-held key, a PKCS#11
  HSM, or a cloud KMS auto-unseal, each keeping the key off the disk that
  holds the data.
- **Choosing it:** the default. `cargo xtask dev --runtime celld` runs it
  (on :8802, its state in `target/devstack/openbao-8802`, which `--clean`
  clears) unless `FRAGMENT_SECRETS_BACKEND=vars`; the e2e on celld unless
  `FRAGMENT_E2E_SECRETS=vars`. `wrangler dev` keeps its own store.
- **A company's Vault:** `FRAGMENT_SECRETS` names it (`{"backend":
  "openbao", "addr": "https://vault.corp.example:8200", "mount": "secret",
  "prefix": "fragment", "namespace": "eng", "secrets": […]}`), each secret
  a KV v2 entry whose field is `value`, the cell's token in
  `FRAGMENT_SECRETS_TOKEN`, its CA in `CELLD_EXTRA_CA_FILE`. Only OpenBao
  is tested.

## Mail

Master mails through Email Sending's binding (`EMAIL`), or in dev and the
e2e through the fake. A self-hosted deployment has neither: `mail::send`
sends nothing, logs one `mail.unsent` warning ("email not sent: this
runtime has no mailer, not yet implemented") and answers no message id,
never an error. An invite still waits for its person's yes; a key is still
added; an org's `mailed` and a trial's `sent` say false. Debt:
docs/technical-debt-ledger.md, "No mail without Email Sending".

## What celld does differently, still true

The old lane's numbering is kept.

8. **`celld dev` stops on SIGINT**, not promptly on SIGTERM: devstack stops
   it so.
21. **celld ties a WebSocket to the request that opened it** (Paul's
    denoland/celld#254): seam 2's.
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
45. **celld dev fences itself when one lease renewal stalls** past its
    10 s TTL under load (`SELF-FENCE: node lease not renewed within TTL`,
    exit 3): seen again 2026-11-11 while a release build ran beside the
    e2e (load 38). The sections after it fail until the next node start;
    on a box under load, run nothing heavy beside a celld stack.

New in this phase:

47. **A streamed request body forwarded to a Durable Object loses its
    length on celld:** master streams a blob upload through the router to
    the fragment's object (FIN-405), whose `content-length` workerd carries
    and celld does not, so every blob upload failed ("a blob upload
    declares its content-length"). The router now forwards the length it
    checked, as the computer's egress already does for the same upload.
    The celld change that would make this parity: an inbound request's
    body keeps its declared length (`_expectedLength`) when forwarded, as a
    service binding's request does.
48. **No `CF-Connecting-IP` on celld:** the cell counts a CLI code's asks
    by it, so every address is `unnamed` and shares one bucket. A front
    proxy that sets the header (a tailnet's, a company's) gives each its
    own.
49. **The dev WorkOS fake's adoption reads wrangler's state layout**
    (`devstack::signed_in_subjects`): on celld it finds no one, so a dev
    stack on celld keeps its fake's subjects in the fake's own file
    alone. A celld state that outlived that file is refused at start
    (`celld::workos_users_kept`; `--clean` starts afresh): a fresh fake
    would hand a known subject to another email, who would sign in as its
    person (Codex's review).
50. **celld caps an object's requests in flight at 64**
    (`CELLD_MAX_CELL_REQUESTS`; past it, 503 "cell request limit
    reached"). Cloudflare queues an object's requests with no such count,
    and one fragment's 120 triggered runs an hour (each a Workflow whose
    step waits on its object) pass it. devstack runs celld with 512, at
    least twice the triggered runs (a const assertion holds it).
51. **celld's `facets.abort` fails with "adopt failed"** for a brain's app
    facet: master aborts the facet to install a newer release, and celld's
    release of it (`__celld.__cell.release(<facet>)`, run by
    `finish_cell_adoption`) throws; `run_internal_snippet` drops the
    exception, so only "adopt failed" reaches the cell, which answers every
    later call to that facet `app_failed` ("abort facet: adopt failed").
    The node also logs "a facet did not close with its root … adopt
    failed" for apps' facets at every generation swap. The `brain` and
    `brain-release` sections fail on celld; other apps' installs (deploy,
    templates, ops) abort their facets fine. A celld issue: the minimal
    change is to carry the snippet's exception into its error (so the
    cause is named), then fix what it names.
52. **An object's alarm years ahead panics celld's node,** at once and at
    every start after: its actor's timer queue refuses the deadline
    (`invalid deadline; err=Invalid`, `actor/effects.rs:252`, tokio's
    `DelayQueue`, which takes about two years at most), and the node
    aborts (SIGABRT). Seen when the e2e's move, its cutoff in 2100, set
    the Migrations object's alarm there. workerd takes such an alarm. Any
    app's alarm that far ahead would do the same, so it is a celld issue
    to fix first: the minimal change is to arm a far deadline at the
    queue's bound and re-arm it when it fires. The move is off on
    self-host (`Need::Move`), so the e2e on celld sets none.
53. **celld runs each object in an isolate of its own,** and the Worker
    over a pool of them (`CELLD_MAX_STATELESS_ISOLATES`, one a core),
    where local workerd runs the Worker and every object in one. What one
    isolate's memory holds (the ledger gate, which the levers forget and
    read) is then another isolate's, as on Cloudflare; one stateless
    isolate does not change it. On celld the e2e
    keeps what callers see (the next call refused, 402) and skips what a
    lever reads of the gate (`Suite::one_isolate`).
54. **A slides presenter's Stop is lost on celld** (the `slides` section,
    every run): the page sets its presence and posts `{live: false}` to its
    channel back to back, and the record never lands. Not explained yet;
    celld's docs say it starts a socket's message handlers in arrival
    order without waiting for the one before, where workerd's input gate
    holds the next event while one awaits storage.

## What a self-hosted org won't have, and its setup steps

**A working list** (2026-11-11), of master at 8bc07e62, for one box on a
tailnet with outbound internet, no inbound internet, and the cell on
celld. Revisit after the tailnet design: it would change steps 1 and 2
(MagicDNS names and a box CA pinned in the apps, the org's own domain
optional) and the iOS row (the app's own trust and tailnet). Sign-in is
seam 4's: tsidp on the tailnet, or the org's own IdP.

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
| Sign-in | Finite's WorkOS AuthKit | an OpenID Connect provider (seam 4): tsidp on its tailnet, or its own IdP; redirect `https://<platform>/auth/callback` |
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
4. Secrets: OpenBao (or the org's own Vault), its unseal key kept
   safe (a TPM, an HSM or a KMS once built); the host secret, 32 or more
   random bytes, among the platform's root secrets there (seam 12).
5. Sign-in: tsidp on the tailnet, or the org's own IdP, with fragment as
   its client (seam 4).
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
# its secrets in .dev.vars instead of OpenBao (a local fleet's convenience)
FRAGMENT_SECRETS_BACKEND=vars CELLD_BIN=… cargo xtask dev --runtime celld
# its text tiers on this machine's own model: a deployment's `models` in a file, its key in another
FRAGMENT_MODELS_FILE=models.jsonc FRAGMENT_MODEL_KEY_FILE=model.key CELLD_BIN=… cargo xtask dev --runtime celld
# sign-in through the OpenID Connect fake (seam 4), or the fake shaped as tsidp
FRAGMENT_SIGNIN=oidc CELLD_BIN=… cargo xtask dev --runtime celld
FRAGMENT_SIGNIN=tsidp CELLD_BIN=… cargo xtask dev --runtime celld
# a real provider: its issuer, its client, its secret in a file
FRAGMENT_OIDC_ISSUER=https://idp.<tailnet>.ts.net FRAGMENT_OIDC_CLIENT_ID=… FRAGMENT_OIDC_CLIENT_SECRET_FILE=… FRAGMENT_OIDC_EMAIL_VOUCHED=on CELLD_BIN=… cargo xtask dev --runtime celld
# the e2e on celld (OpenBao behind the stand-ins unless FRAGMENT_E2E_SECRETS=vars)
FRAGMENT_E2E_RUNTIME=celld CELLD_BIN=… cargo xtask e2e --only auth,create,…
```

## Not in this phase

- Computers (seam 2): none on celld here. celld's Docker engine has no
  egress intercepts or snapshots, which a computer needs; its krun engine
  (sandcastle's) has both, and is the next phase's.
- The LAN mode and Dex (seam 9): a tailnet front door replaces them.
- BYOC pairing, own hardware in points (10), cards (7), macrofiche (5).
- Model URLs under `*.localhost` (the old found 36, which Paul answered
  yes): master's rule takes `http://127.0.0.1:<port>` alone.
