# Secrets

Every secret has one home, and code holds a capability, never a key. A
secret is never in git, a log, an answer, a page, a command line or a
channel record, and is never printed. Rotating one means changing it in
its home; everything that uses it reads it from there.

## Where each lives

| Secret | Home | Set and rotated by |
|---|---|---|
| The deployment's own: the host secret, the code.storage org key, WorkOS's client id and API key, Stripe's restricted key and webhook secret, each model slot's key on an OpenAI-compatible API, each operator key, each connection's OAuth client secret | the account's Cloudflare Secrets Store, bound to the platform Worker by name (below) | Paul, or the coordinating session, with `cargo xtask secret`; deleting one is Paul's alone |
| A fragment's secret (an app's API key, a webhook signing key) | the fragment's Durable Object, sealed | its editors (docs/api.md), who may set and list secrets but never read one; a connector's, its owner alone (`cell/src/doors.rs`) |
| A person's own key for an `own` provider | their computer's Durable Object, sealed | the person; one with a sign-in is exchanged by the platform and sealed at once, never pasted; revoking it is the person's, at the provider |
| A sign-in under way (its PKCE verifier) | sealed in the computer (an own key's) or the registry (a connection's), until used or ten minutes pass | no one |
| A fragment's nostr key (an agent's too) and its push key | made and sealed in the fragment's Durable Object, opened only to sign | no one |
| A person's connections (Google, …) | WorkOS Pipes, which holds and refreshes them; a computer's swap asks it for a short-lived token on each call (decision 22) | the person (docs/pipes-browser-binding.md) |
| Browser sessions | the registry, as SHA-256 hashes; the tokens only in HttpOnly cookies | no one |
| The DNS token, a preview's test secret, the Stripe key xtask uses | files on the deploying machine, named by path in the config (Files, below) | Paul |
| An operator key (a wipe's) | a file on the operator's machine (below) | its operator |

A computer's guest holds only placeholders (docs/computers.md). A
placeholder's tag is an HMAC of (computer, agent, provider) under a key
derived from the host secret and never stored, so one that leaks works
only from that computer's egress, toward that provider.

## How code reaches a secret

- **Sealed at rest** by `crates/core/src/seal.rs` through
  `cell/src/keys.rs`: AES-256-GCM under a key HKDF'd from the host secret
  and salted with the sealing Durable Object's class and id, so a value
  opens only where it was sealed.
- **`cell/src/keys.rs` is the one place a store secret is read**, through
  a per-isolate cache (`secrets_store::CACHE_MS_MAX`): a value set again
  is in use within a minute, with no deploy. A binding whose secret the
  store lacks is an error; nothing falls back to a Worker secret or
  variable.
- **App code holds no key.** It runs in a loader isolate whose env the
  platform builds (`js::app_env`), so it names no deployment secret. A
  job's `job.fetch` names a fragment secret as `{{NAME}}` in a header or
  in its URL's path or query, and the platform fills it as the request
  leaves, over https and only toward a host the secret names
  (`fragment_core::secrets::may_fill`). A value filled into a URL that
  would change its scheme, host, port, path segments or query parameters
  is not sent (`fill_url`). A failure's message and the answer name every
  value the fetch filled, a header's or the URL's, as its placeholder
  again, in each form a vendor writes one back in: as it is,
  percent-encoded (either hex case, once or twice), JSON-escaped (also
  within JSON), HTML-escaped, and in base64 for a value of 12 bytes or
  more (`redact`; #814). A value that comes back compressed, hashed or
  cut in pieces by the vendor is past what matching text can name. Its hosts are set only with
  its value, so an editor who cannot read a value cannot send it anywhere
  its setter did not mean. An editor sets one through the API (`fragment
  secret set`) or from the fragment's own page (`PUT __secrets/{KEY}`,
  the same contract over its session), so a connector's page takes a
  pasted token, and makes a webhook's secret in the browser, without
  either passing through app code or a model.
- **Agents hold no key.** Model calls go through the platform's model
  route, which adds a deployment key toward that API alone; operator keys
  and connections are swapped in at the computer's egress.

## The Secrets Store

The deployment's config names each secret by its store name. `cargo
xtask deploy` binds it under a name fixed in `fragment_core::secrets_store`,
reads no value, and refuses before building when the store lacks one the
config names. The only Worker secrets a deploy uploads are a branch's
test secret and its own Stripe webhook secret (decision 58).

Values go in only through `cargo xtask secret set|gen|list`
(xtask/src/secret.rs): from wrangler's hidden prompt, standard input, or
a file read once, never `--value`. There is no `rm`: deleting a secret is
irreversible, and Paul's. Dev and the e2e bind the same names in
wrangler's local store, which devstack seeds.

**Rotating a key**: `secret set` it again, then revoke the old one at its
vendor once a minute has passed.

### Rotating the host secret

By name, never in place: every value at rest is sealed under it and the
store gives no value back, so `set` refuses the config's host secrets.
Make a new one (`cargo xtask secret gen fragment-host-secret-<date>`),
name it `host_secret` and the old one `host_secret_previous` in the
config, and deploy. Values sealed under the old one open, and are
resealed as they are read; placeholder tags under it still verify. Once
that has run its course, remove `host_secret_previous` and deploy again.

## Files

Three secrets are files named by path in a config, because xtask itself
uses them or the store would never give them back: `dns_token_file`,
`test_secret_file` (below), and `stripe.key_file` (xtask's Stripe calls).

CI's runners hold no files of their own. Each GitHub environment
(`preview`, `production`) holds `CLOUDFLARE_API_TOKEN` (also written as
the DNS token file), `DEPLOY_REPO_TOKEN` (read-only on
finitecomputer/fragment-deploy, whose configs hold names and paths,
never a value), `CLOUDFLARE_ACCOUNT_ID` (a secret so that logs mask it)
and the preview's `PREVIEW_TEST_SECRET`; production's also holds
`PREVIEW_REGISTRY_TOKEN` and `PREVIEW_ACCOUNT_ID` for copying an image
from the preview's registry. The merge queue's image build has an
environment of its own, `preview-image`, open only to master's queue
(`gh-readonly-queue/master/*`), whose token has the preview account's
Containers permission alone. Paul sets them. Registry credentials the
image jobs mint are never printed.

## The test secret

A preview's test levers (docs/api.md, Test levers) exist only where a
test secret is. Its home is the config's `test_secret_file` (32 random
bytes or more), uploaded as the Worker secret `FRAGMENT_TEST_SECRET` on a
`--branch` deploy only: a deployment of its own refuses one at deploy and
honours none at run time. The hosted e2e sends it in
`x-fragment-test-secret` to the platform's host alone. It reaches only
e2e people (`<name>@e2e.test`) and `e2e-…` fragments, within a daily
limit, so one that leaked touches no one real. Rotate it by changing the
file and deploying the branch. Dropping the file from the config leaves
the Worker secret as it was: remove it with `wrangler secret delete
FRAGMENT_TEST_SECRET --name fragment-<branch>`.

## Operator keys (a wipe's)

The deployment lists operators' npubs (`operators`), never their
secrets. An operator's key is a file of 64 hex, 0600, that `fragment
operator key <file>` makes, read by path (`--key-file` or
`FRAGMENT_OPERATOR_KEY_FILE`). To rotate one, make a new one, list its
npub and deploy, then remove the old npub and deploy. A key that leaked
wipes anyone until its npub is removed.
