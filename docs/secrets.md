# Secrets

Every secret has one home, and code holds a capability, never a key. A
secret is never in git, a log, an answer, a page, a command line or a
channel record, and is never printed. Rotating one means changing it in
its home; everything that uses it reads it from there.

## Where each lives

| Secret | Home | Set and rotated by |
|---|---|---|
| The deployment's own: the host secret, the code.storage org key, WorkOS's client id and API key, Stripe's restricted key and webhook secret, a read-only Workers Observability key (the operators' glance reads the account's Workers Logs with it: an account API token with Workers Observability Read and nothing else), each model slot's key on an OpenAI-compatible API, each operator key, each connection's OAuth client secret | the account's Cloudflare Secrets Store, bound to the platform Worker by name (below) | Paul, or the coordinating session, with `cargo xtask secret`; deleting one is Paul's alone |
| A fragment's secret (an app's API key, a webhook signing key) | the fragment's Durable Object, sealed | its editors (docs/api.md), who may set and list secrets but never read one; a connector's, its owner alone (`cell/src/doors.rs`), and removed when it installs other code (a fork: Paul, 2026-10-31) |
| A SimpleX connector's inbox secret, as its door delivers with it | the connector's door (cell/door.mjs), in its Durable Object's own storage, unsealed, as a webhook's sender holds one: given by the connector's `connect`, filled at the egress, never told back | its owner, setting the connector's `SIMPLEX_DOOR_SECRET` and connecting again |
| A person's own key for an `own` provider | their Vault, sealed (below; M16: a computer from before hands its own over once) | the person; one with a sign-in is exchanged by the platform and sealed at once, never pasted; revoking it is the person's, at the provider |
| A person's secret for their agents (a token, a bot's, a Nostr key) | their Vault, a Durable Object of theirs, sealed (below) | the person alone, never an agent: `PUT /api/vault/{secret}` (docs/api.md, "A person's Vault"); or the platform's own code for them (`vault::put`: FIN-463's hand-off) |
| A sign-in under way (its PKCE verifier) | sealed in the person's Vault (an own key's; one begun before the move, in their computer) or the registry (a connection's), until used or ten minutes pass | no one |
| A fragment's nostr key (an agent's too) and its push key | made and sealed in the fragment's Durable Object, opened only to sign | no one |
| A person's connections (Google, …) | WorkOS Pipes, which holds and refreshes them; a computer's swap asks it for a short-lived token on each call (decision 22) | the person (docs/pipes-browser-binding.md) |
| Browser sessions | the registry, as SHA-256 hashes; the tokens only in HttpOnly cookies | no one |
| The DNS token, a preview's test secret, the Stripe key xtask uses | files on the deploying machine, named by path in the config (Files, below) | Paul |
| An operator key (a wipe's) | a file on the operator's machine (below) | its operator |
| A self-hosted box's identity door: its own secret (it seals the door's state at the provider and derives each sign-in's PKCE verifier and nonce) and its client secret at the OpenID Connect provider | files on the box, owner-only (the door refuses one others may read), named by its config (docs/self-host.md, seam 4); the platform's client secret at the door stays in the node's secrets backend as `WORKOS_KEY`, and the door's config holds only its SHA-256 | the box's operator |

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
  A self-hosted runtime (celld) binds them the same way and reads each
  from its node's secrets backend, OpenBao (docs/self-host.md, seam 12):
  the cell is the same on both.
- **App code holds no key.** It runs in a loader isolate whose env the
  platform builds (`js::app_env`), so it names no deployment secret. A
  job's `job.fetch` names a fragment secret as `{{NAME}}` in a header or
  in its URL's path or query, and the platform fills it as the request
  leaves, over https and only toward a host the secret names
  (`fragment_core::secrets::may_fill`). A value filled into a URL that
  would change its scheme, host, port, path segments or query parameters
  is not sent (`fill_url`). A failure's message and the answer name every
  value the fetch filled, a header's or the URL's, as its placeholder
  again, in each form a vendor writes one back in, character by
  character: as it is, its UTF-8 percent-encoded (either hex case, once or
  twice), JSON-escaped (`\u` past ASCII and surrogate pairs, a control
  character's short escape such as `\n`, also within JSON), as HTML character references; and in base64 for a value of 12
  bytes or more, itself in any of those forms (`redact`; #814). A value that comes back compressed, hashed or
  cut in pieces by the vendor is past what matching text can name. Its hosts are set only with
  its value, so an editor who cannot read a value cannot send it anywhere
  its setter did not mean. An editor sets one through the API (`fragment
  secret set`) or from the fragment's own page (`PUT __secrets/{KEY}`,
  the same contract over its session), so a connector's page takes a
  pasted token, and makes a webhook's secret in the browser, without
  either passing through app code or a model.
- **No deployment traces its fetches**: a Workers trace records each
  fetch's URL and query, which may carry a secret filled at the egress,
  so `cargo xtask deploy` pins `observability.traces` off beside its logs
  (#814).
- **Agents hold no key.** Model calls go through the platform's model
  route, which adds a deployment key toward that API alone; operator keys
  and connections are swapped in at the computer's egress. Each operator
  key is locked at its vendor (scopes and a spend limit where offered),
  and its catalog row's `deny` names the account-wide calls the vendor
  can't scope away, which the swap never sends it on (docs/computers.md,
  "Calls an operator key never goes on").

## A person's Vault

FIN-466: a person keeps secrets on the platform, and their agents use
them without holding them. One `Vault` Durable Object a person, named by
their identity (cell/src/vault.rs), holds each by its name: its value
sealed for that object, the hosts it may go to, where it goes on a
request, and the agents that may use it (Paul, 2026-11-09: per agent).

- **A value goes in and never comes out.** No answer, list, log, event or
  wipe report names one; the person who set it cannot read it back.
- **Only its person says where a value goes** (`fragment_core::vault::check`):
  its hosts, place and agents, with its value or, set again with none,
  keeping the one held. A kept value keeps its kind: a Nostr key stays a
  signer and a token a fill, so a new secret, or one going between the
  two, needs a value. So a key handed off parked is pointed at its host
  with no one carrying it by hand. No hosts: parked, it fills nothing.
- **Only its person writes it**, from their session or their own key;
  their agents are refused every route. The CLI's `fragment vault
  set|list|rm` reads a value from a hidden prompt, or stdin, never argv or
  the environment, and never prints one; `set … --keep-value` reads none.
- **An agent uses one through its computer's swap**, as an own key: its
  view lists each secret bound to it with a host, as `fck_<name>_<tag>`
  toward its hosts, and the computer's egress catches those hosts (armed
  again as the view is read). For a request carrying one, the computer
  asks the Vault (`vault/fill`), which answers only for an agent the
  secret names, toward one of its hosts; the swap plans the request with
  that secret's row beside the catalog (`Catalog::with`). A request to
  those hosts carrying no placeholder goes on as it came, its own
  `Authorization` (a git remote's basic auth) untouched.
- **At most 8 hosts** (`fragment_core::vault::room`): a computer's egress
  takes 128 intercept entries, each host 4 (HTTP and HTTPS), so the
  platform's own, the catalog's (at most 21) and a Vault within its 8
  always fit at a start (`fragment_core::computer::egress`). A secret that
  would send the Vault to a ninth distinct host is refused (400), with a
  value or without, in words that say so; a host another secret goes to
  costs nothing more. A catalog's host counts as any other, since a
  computer registers each Vault host as its own, so a deploy that changes
  the catalog never moves a Vault over its share. A Vault over the cap from
  before it is refused only a host it does not go to yet, so each secret
  may still be set again as it is, or narrowed.
- **A Nostr key signs, never leaves** (`place: {"sign": "nip98"}`): the
  Vault answers a fresh NIP-98 event over the request as it goes (its
  https URL, method and body's SHA-256) in `Authorization: Nostr …`, and
  such a request carries no other placeholder.
- **Echoes are hidden**: an answer to a request a Vault filled is read
  whole (text, at most 8 MiB) and each value it echoes named `{{name}}`,
  as a job's fetch names its own; one that streams as it is made (an event
  stream, newline-delimited JSON, audio, video, an image, octet-stream) is
  named as it streams, each value's own bytes. The operator's keys on the
  request are named first, in a pass of their own, so a value its person
  chose never cuts into one (`secrets::redact_passes`).
- **Their own keys are its too** (M16), each its own kind (Paul,
  2026-11-18): one per `own` provider of the catalog, used by every agent
  of theirs not narrowed from it, as before. Its person sets or removes
  one (`PUT`/`DELETE /api/connections/{provider}/key`, or a sign-in's
  finish, whose verifier the Vault keeps sealed ten minutes); a removal is
  kept as one. Their computer's swap asks the Vault for the key as each
  request goes, beside its ask of the agent's fragment, and the view lists
  which they hold. Neither asks a computer.
- **A computer from before the move hands its own keys over** once, at
  its owner's next read of it or of their connections, or its agent's
  next call through one (never a start, an exit, a takeover or its
  guest's request): it opens each, sends it to their Vault in one
  hop between the two, logs only providers and counts, and deletes it
  whether the Vault took it or not. The Vault takes one only where its
  person set or removed none since, so a key sent late, or again, never
  replaces what they said. One the Vault did not answer for stays owed. A
  sign-in begun on a computer before the move finishes from there.
- **A wipe empties it** with their ledger, and keeps the row that says so.
- **An old finite.computer agent's keys come in once** (FIN-463 F11;
  docs/migrations.md, "The secrets hand-off"): on its person's Yes, the
  move puts each into their Vault, bound to the new agent
  (`vault/handoff`), and keeps a receipt in the same turn, so a retry
  never puts back one they removed or moved since. A hand-off never
  takes the name of another agent's secret.
- **A secret moves out once, whole**: its person may move one into a
  fragment they edit as that fragment's secret (`fromVault` on `PUT
  /api/f/{name}/secrets/{KEY}`; `fragment secret set … --from-vault`), as
  a Telegram bot's token moves into its connector. The Vault claims the
  value for that one destination as the move reads it; the fragment keeps
  it, then the Vault removes it, for its claimant alone. So it has one
  home and is never lost between: a move into anywhere else is refused
  (409) while the claim stands, and the same move again finishes one cut
  short. A claim has no clock; setting the secret again with a value, or
  removing it, ends it (`fragment_core::vault::Claimed`); set again with
  no value, the value claimed is the one held, and its claim stands. A Nostr key never moves.
- **A move is its person's word, never a page's**: from the platform's
  own pages (the shell's session) or their own key, never from a
  fragment's page (`__secrets`, a site or frame session: 403 for any
  name, before the Vault is asked). A page runs its authors' code with its
  visitor's session, and an author may be the visitor's own agent, so a
  page's call says nothing of what they want. The Telegram connector's
  setup page asks Settings instead (`vault-move`, frame-messages.js):
  Settings moves only the bot it offered that frame, into that connector.
  A move keeps the secret within the hosts its Vault names
  (`fragment_core::vault::within`, 400 otherwise, before it is claimed);
  a parked one's move names them.

### An agent asks

An agent that needs a secret asks its person for it (`fragment vault ask
<name> --why "…" [--to <host>]`; docs/api.md, "A person's Vault"), the
way T3 Code's secret prompt does: the agent never sees what they give.

- **The card is the platform's.** The ask waits in the person's Vault and
  shows as one of their notices (`vault/notices`), in the band the shell
  draws above their chat, and at its own link on the platform
  (`SecretAsk.url`, `/vault/ask/{id}`): an agent chatting through Telegram
  or SimpleX asks with `--no-wait`, sends the link, and waits with
  `fragment vault wait <id>`. A client that draws no field (iOS today)
  is sent that link (Open) in place of the field and the grant, so
  nothing is granted from a card that may hide what it grants. A chat, an
  app or a fork of either draws no part of the card, so no app code ever
  has the field.
- **The paste goes to the Vault alone**: from the shell's page to `POST
  /api/vault/asks/{id}`, sealed there in the same turn, never in a chat's
  records, a log or an answer. The page keeps it in its input only while
  that card shows (a card whose grant changed is a new, empty field), and
  empties it once posted.
- **Only the person asked answers**, from their own session; an agent,
  theirs or another's, is refused, and anyone else finds no ask.
- **A new secret, or one already kept.** For a new name (`held: "none"`)
  the card has the masked field, Save and Decline: Save makes the secret
  toward the hosts asked, for the asking agent alone. For a name the
  person keeps, it has Allow and Decline and no field, and says what an
  Allow adds (this agent, new hosts, and that its other agents could send
  there too): Allow keeps the sealed value and its place. Either answer
  carries the secret's revision as the card showed it (`held`); one that
  changed since is refused (409), checked again after a seal. An ask is
  checked as its answer's write will be, and refused as the agent asks,
  in the same words, with no card drawn, when that write would be: a new
  secret in a Vault that keeps 64, an Allow past a secret's own 8 hosts or
  16 agents, or either taking the Vault past its 8 hosts (above: every
  distinct one, the catalog's too). No one is asked to answer what the
  write would refuse. So a refusal can tell an agent its person's Vault is
  full, or that it keeps a name: an ask reaches only the asking agent's
  own person's Vault, and their agents may see everything they have
  (decision 44), never a value. The person changes or removes a secret in
  Settings → Secrets, each change or removal made only onto the secret as
  its sheet shows it (`held`, 409 otherwise, and the sheet is drawn again
  from what the Vault holds), or with `fragment vault set|list|rm`.
- **It ends**: saved, declined, or after ten minutes unanswered (or asked
  again by the same agent for the same name). At most 8 wait on a person,
  and their Vault keeps at most 64, ended ones an hour.
  The agent's CLI then reads its computer's view, which arms the egress
  for the new hosts, and prints the stand-in (`fck_<name>_<tag>`).

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
from the preview's registry. Paul sets them. Registry credentials the
image jobs mint are never printed.

## The test secret

A preview's test levers (docs/api.md, Test levers) exist only where a
test secret is. Its home is the config's `test_secret_file` (32 random
bytes or more), uploaded as the Worker secret `FRAGMENT_TEST_SECRET` on a
`--branch` deploy only: a deployment of its own refuses one at deploy and
honours none at run time. The hosted e2e sends it in
`x-fragment-test-secret` to the platform's host alone. It reaches only
e2e people (`<name>@e2e.test`) and `e2e-…` fragments, within a daily
limit; the levers that add a member or post as one act for e2e people
and their agents alone, on fragments e2e people own, so one that leaked
adds no real person anywhere and posts as no one real. Other fragment
levers check only the `e2e-` label, which is not reserved (docs/api.md,
Test levers). Rotate it by changing the
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
