# Secrets and encryption (FIN-590)

How secrets are kept, how data is encrypted in transit and at rest, and
how keys are rotated. The contract is docs/secrets.md; this page turns
it into answers and adds what it does not cover.

## Secret custody

Every secret has one home, and code holds a capability, never a key. A
secret is never in git, a log, an answer, a page, a command line or a
channel record (docs/secrets.md, first paragraph).

| Secret | Home | Who sets and rotates it |
|---|---|---|
| The deployment's own: the host secret, code.storage's org key, WorkOS's client id and API key, Stripe's keys, the Workers Observability read key, each model key, each operator key, each connection's OAuth client secret | Cloudflare Secrets Store, bound to the Worker by name | Paul, or the coordinating session, with `cargo xtask secret` (`xtask/src/secret.rs`); deleting one is Paul's |
| A fragment's secret (an app's API key, a webhook key) | the fragment's Durable Object, sealed | its editors, who may set and list but never read one |
| A person's secrets for their agents, and their own keys (OpenRouter) | their Vault Durable Object, sealed | the person alone, never an agent |
| A person's nostr key | the Registry, sealed (decision 46), never in WorkOS | no one |
| A fragment's or agent's nostr key and push key | made and sealed in its Durable Object, opened only to sign | no one |
| A PKCE verifier for a sign-in under way | sealed in the Vault or the Registry, ten minutes | no one |
| A person's connections (Google) | WorkOS Pipes, which holds and refreshes the tokens | the person |
| Browser sessions | the Registry, as SHA-256 hashes; the tokens only in HttpOnly cookies | no one |
| CI's deploy credentials | GitHub environments `preview` and `production` | Paul |
| Files a deploy reads: the DNS token, a preview's test secret, xtask's Stripe key | files on the deploying machine, named by path in the config | Paul |
| An operator key (for a wipe) | a 0600 file on the operator's machine | its operator |

Source: docs/secrets.md, "Where each lives", "Files".

Key points for the questionnaire:

- **One reader.** `cell/src/keys.rs` is the one place a store secret is
  read. A binding whose secret is missing is an error; nothing falls
  back to a Worker variable.
- **Values go in only through `cargo xtask secret set|gen`,** from a
  hidden prompt, stdin or a file read once, never `--value` on a command
  line. There is no `rm`.
- **Deploy configs hold names, never values.** The private
  finitecomputer/fragment-deploy repo holds store names and file paths
  (docs/secrets.md, "Files").
- **App code holds no key.** It runs in a loader isolate whose env the
  platform builds. A job's fetch names a secret as `{{NAME}}`; the
  platform fills it on the way out, over https, only toward the hosts
  the secret names (`fragment_core::secrets::may_fill`).
- **Agents hold no key.** Computers hold placeholders; the egress swaps
  in the real credential toward the provider's hosts alone
  (docs/computers.md, "Connections and operator keys").

## Encryption in transit

| Path | How | Source |
|---|---|---|
| Browser to `finite.chat` and `*.finite.place` | HTTPS at Cloudflare's edge. One wildcard certificate covers every fragment (R16). Session cookies are `__Host-`, HttpOnly. | decision 5, R16; HSTS and TLS minimum: http-headers.md (in progress), FIN-575, FIN-577 |
| CLI and agents to the platform | HTTPS, each request NIP-98 signed | R15; `crates/nip98` |
| The cell to WorkOS, Stripe, code.storage, Google's token endpoint, model APIs | HTTPS from the Worker | `cell/src/keys.rs` (`confidential_request`); `cell/src/models.rs` |
| A computer's guest to a provider (Google, an operator key's vendor) | The guest's HTTPS is caught by the Computer DO's intercept, signed by Cloudflare's containers CA, which the image trusts; the intercept sends on over HTTPS. It catches HTTP to those hosts too and sends that over HTTPS. | docs/computers.md, "Environment", "The intercept" |
| A computer's guest to the platform (`api.fragment.internal`, `model.fragment.internal`) | caught by the Computer DO's intercept inside Cloudflare's container runtime; not sent to the internet | docs/computers.md, "Environment" |
| A job's fetch with a filled secret | refused unless https | docs/secrets.md, "How code reaches a secret" |
| Web push | encrypted for the device (Web Push) | `cell/src/push.rs` |

## Encryption at rest

### What Finite seals itself

`crates/core/src/seal.rs`, through `cell/src/keys.rs`: AES-256-GCM under
a key derived (HKDF) from the host secret and salted with the sealing
Durable Object's class and id. A value opens only in the object that
sealed it. Sealed with it: fragment secrets, Vault values, a person's
key, fragments' keys, PKCE verifiers.

Session tokens are stored as SHA-256 hashes, so a copy of the Registry
holds no usable session.

### What rests with providers

Finite does not add its own encryption to people's content (chats,
apps, files, an agent's memories; decision 30 says the same of brains).
That content rests encrypted only as each provider encrypts its storage.

| Store | What | Encryption at rest |
|---|---|---|
| Durable Objects' storage | members, channels (chats), app SQLite, Registry, Vault, Ledger | Cloudflare's (To confirm the exact statement to cite) |
| R2 | large files' bytes, exports, computers' saves of `/data` | Cloudflare's (To confirm) |
| Container snapshots | a sleeping computer's files | Cloudflare's (To confirm); no delete, expire within 30 days (docs/computers.md) |
| Workflows, Queues | job steps; deliveries | Cloudflare's (To confirm) |
| code.storage | every fragment's git, each agent's self | To confirm with code.storage |
| WorkOS Pipes | connection tokens (Google) | To confirm with WorkOS |
| Model providers | nothing kept: zero data retention, AI Gateway logs off | decision 23; privacy policy (OpenRouter) |

## Key rotation

From docs/secrets.md:

- **A store secret:** `cargo xtask secret set` it again; the cell's
  per-isolate cache picks it up within a minute, with no deploy; then
  revoke the old one at its vendor.
- **The host secret:** by name, never in place. Make a new one, name it
  `host_secret` and the old one `host_secret_previous`, deploy. Values
  sealed under the old one still open and are resealed as they are
  read. Once that has run its course, drop the old one and deploy again.
- **An operator key:** make a new one, list its npub, deploy, remove the
  old npub, deploy. A leaked one works until its npub is removed.
- **The preview test secret:** change the file and deploy the branch.
- **A person's connection:** the person disconnects and connects again;
  a disconnect revokes the grant at the provider.

There is no rotation schedule in the repo.

## Gaps

- No written rotation schedule for the deployment's own secrets.
- No application-level encryption of people's content beyond what the
  providers give.
- Provider encryption at rest is assumed, not yet cited.

## To confirm

- Alex: the Cloudflare documentation or SOC 2 report to cite for
  encryption at rest of Durable Objects, R2, container snapshots,
  Workflows and Queues.
- Alex: code.storage's and WorkOS Pipes' encryption at rest, from their
  security pages or reports.
- Paul: whether we commit to a rotation schedule (for example yearly for
  the host secret and vendor keys) or rotate on events only.
- Paul: when the host secret was last rotated, if ever.
