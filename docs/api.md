# fragment wire contract

The cell (`cell/`, Rust on Cloudflare Workers) answers every route below;
the CLI, the browser library and computers' images are its clients, and
`cargo xtask e2e` proves each. A limit in capitals is
`fragment_proto::limits`'s unless another module is named; its value is
there.

Errors are `{"error": "<code>", "message": "..."}` (`proto::ErrorCode`).
Billing's, seats' and orgs' refusals add `"reason"` (`proto::Reason`)
where one status has several causes a person acts on differently. It is
add-only: a reason a client does not know reads as none.

Any route may answer:

- 503 `deploying` while a deploy goes out: a call between the Worker and
  one of its objects met another release, which refused it before doing
  anything (docs/MODEL.md, Calls between releases). Nothing was done, so
  the same request again is safe. A page navigation (by Fetch Metadata,
  else its `Accept`) gets a small HTML page instead: still 503, with
  `Retry-After: 3` and `no-store`, reloading itself after three seconds.
- 500 `schema_refused`: a platform object refuses its code's schema
  (docs/MODEL.md, Schemas); the message names the object and why.

## Configuration

The Worker's variables are `cell/src/config.rs`'s, each documented
there, rendered from the deployment's config by `cargo xtask deploy`
(`deploy/example.jsonc`; `.dev.vars` in dev). Its secrets are Secrets
Store bindings read only by `cell/src/keys.rs` (docs/secrets.md), and its
other bindings are `cell/wrangler.jsonc`'s. An app's isolate gets an env
the platform builds, so no author code can name a binding.

Every answer of the platform's host names its release in
`x-fragment-deploy` (`FRAGMENT_DEPLOY_ID`), and, when CI published and
verified a CLI for the release's inputs, that version in `x-fragment-cli`.

A request body is at most what the zone's Cloudflare plan takes (100 MB
on Free and Pro), below the `BLOB_MAX_BYTES` a blob route allows (debt
ledger).

## Test levers

`/api/test/*` (cell/src/levers.rs) exists only where the Worker secret
`FRAGMENT_TEST_SECRET` is honoured: a branch deployment
(`FRAGMENT_HOST_LABEL_SUFFIX`) or a local fleet (`FRAGMENT_EGRESS_LOCAL`),
and only when it is 32 to 256 bytes of printable ASCII. One set anywhere
else, or too short, is logged (`levers.refused`) and honoured nowhere. A
lever answers only a request carrying the secret in
`x-fragment-test-secret` (compared in constant time); any other request
gets the 404 a missing route gets, word for word. On a branch the levers
reach the e2e's own things alone: a fragment lever only an `e2e-…`
fragment, a ledger or computer lever only an e2e person's (403
otherwise); the registry's and the mail lever are no route.

| Route | Body |
|---|---|
| `POST /api/test/signin` | `{email, paidCalls?}` → `{session, identity, created, paidCalls}`: a platform session for the e2e person `<name>@e2e.test` (issuer `e2e.test`, which no real sign-in has), made a seat, their paid calls capped at `paidCalls` (at most `levers::E2E_PAID_CALLS_MAX`; absent, unchanged). A branch makes at most `levers::E2E_PEOPLE_DAILY_MAX` e2e people and lends `levers::E2E_PAID_CALLS_DAILY_MAX` paid calls a UTC day (429) |
| `POST /api/test/people` | `{after?}` → `{people: [{identity, email}], next}`, 100 a page |
| `POST /api/test/ledger` | `{identity, op, …}` |
| `POST /api/test/keys` | `{fragment, op, plaintext \| sealed}` |
| `POST /api/test/list` | `{identity, op}` |
| `POST /api/test/fragment` | `{fragment, op, …}` |
| `POST /api/test/computer` | `{computer, op, times?, on?, ms?, freeze?}` |
| `POST /api/test/export` | `{identity, op, steps?}` |
| `POST /api/test/mail` | `{to, subject, text}` → `{messageId}` |
| `POST /api/test/registry` | `{…}` |

Each op is its cell's to define; an unknown one, or one without its
argument, is 400. One hook is no route: on a local fleet with a test
secret, a request whose `fragment_e2e_release` cookie names a release is
answered as that release (`x-fragment-deploy`, the shell's meta), so a
browser meets a deploy (`answered_release`).

## Mail

The platform's own mail (cell/src/mail.rs, `fragment_core::mail`):
transactional only, one plain-text message to one plain address, from the
deployment's `FRAGMENT_MAIL_FROM`, through Email Sending's `EMAIL`
binding. A sender is a bare address or an unquoted `Name <address>`; any
other is refused at config read and before a deploy. A subject is 1 to
200 characters on one line and a text at most 16 KiB; anything else is
400 and nothing is sent. The service's refusals keep its `E_…` code in the
message: too many sent is 429, a message it will not take 400, the rest
502. Unset, the deployment sends no mail. A message to an e2e person
(`@e2e.test`) on a preview goes nowhere.

It sends invites (Sharing), seats and trials (Billing), a finished export
(Export), and a key added to a person (Sign-in): the key's ending, the
whole key, how it was added, and `fragment keys revoke`, after the answer,
whatever becomes of the mail. Each names the product as the shell does
(`fragment_core::mail::PRODUCT`).

## Principals and access

A principal is an identity (a person or an agent, named by the npub of
the key it was made with: decision 45) or an anonymous visitor (`anon:` +
32 hex, the hash of a random cookie on the fragment's origin). A fragment's
own key is the principal of its triggered runs, and is not registered.

A request is signed by a key (64 hex, or an npub). The router asks the
registry, live, which identity holds it on every signed control request: a
key no one registered, or a revoked one, is 401; a registry that cannot
answer is 503 `registry_unavailable`, never let through. A site request's
signature or session cookie is resolved only when the answer depends on
who asks: a page or file everyone who may see the fragment gets alike (a
`public` fragment's, a `link` fragment's with its share link) is served
without the registry. An operation, a socket, an invite, push, an app
route, and any read the anonymous standing may not see ask it live.

NIP-98 (`crates/nip98`): `Authorization: Nostr <base64 event>`, kind 27235,
empty content, tags `["u", <absolute URL>]`, `["method", <method>]` and,
for a body, `["payload", <hex SHA-256 of it>]`; `created_at` within
`AUTH_WINDOW_S` of the cell's clock; `id` and BIP-340 `sig` as NIP-01.
Every field is required and typed. A `payload` binds the body even when it
is empty. A blob upload's signature binds its URL, which names the bytes'
hash: its `payload` may name that hash or be absent; any other hash is 401.

Bodies the router reads are at most `BODY_MAX_BYTES`; a model call's
(`chat/completions`, `decide`) `models::MODEL_BODY_MAX_BYTES`; a transcription's
`transcribe::BODY_MAX_BYTES`; an embedding's `embed::BODY_MAX_BYTES`. Each is measured as it arrives: a longer
`content-length`, or a chunked body past it, is 413 before anything is
authenticated.

Roles, weakest first: `public`, `viewer`, `editor`, `owner`. A member's
role is their membership. Otherwise visibility decides: on a `public`
fragment everyone holds `public`; on a `public` or `link` fragment the share
link (`?view=<token>`, which sets the HttpOnly `fragview` cookie on the
fragment's origin) is `viewer`; a `members` fragment gives non-members
nothing. A refusal is 401 to an unsigned caller and 403 to a signed one.

The share link's token never stays in a URL a page's code reads: a page
navigation that brings it (Fetch Metadata `navigate`, `Accept` naming
HTML) is a `302` (`no-store`) setting the cookie, on to the same path
(`site::way_on`) and query without `view`. A request with no Fetch Metadata
(an unfurler) reads the page where it asked. A fetch, an image or a socket
naming `?view=` still counts. An app's `fetch` is never handed `view`. A
fragment's origin serves no worker script but the platform's `__sw.js`
(any other `Service-Worker: script` request is 404), so no app registers
one that would see the navigation first. `fragview` does not count in a
frame (Which cookies count).

**An agent's owner reads what it reads**: an owner who is not a member,
but whose agent is, reads the fragment as a `viewer`, and acts through it
never (a write needs their own membership: 403).

**An agent acts for whoever asked, capped** (R17). A request signed by an
agent may name `for=<npub>` in its signed URL's query: it acts with the
lower of that identity's role (its membership, an agent of its own that is
a member, or the visibility floor; never a share link) and the agent's cap
(its own role, or its owner's membership role if higher; never above
`editor`). The principal stays the agent; an operation called `for`
someone sees `call.principal` as them and `call.agent` as the agent. `for`
is honoured on `/api/f/{name}/…`, on `GET` and `POST /api/fragments` (a
`POST` for anyone but the agent's owner is 403), on `POST /api/search`
(each fragment asked decides as on its own route) and on
`/api/models/v1/…`; anywhere else 400. A person naming `for` is 403; two, or one that is no
identity, 400. A site request's query is its app's.

**An agent shares for its owner** (`fragment_core::access::agent_shares`):
acting `for` its owner, and not held below them, on a fragment its owner
owns, an agent manages members, invites, visibility and rotation as its
owner would (never granting `owner`, never touching the owner). Any other
agent's sharing request is 403, saying why. Deleting a fragment, setting
its cap and sharing a connector (An agent's doors) are 403 to every agent.
What an agent shares names it (`addedBy`,
`createdBy`), and each event (`member.set`, `member.removed`,
`invite.created`, `invite.revoked`, `visibility`, `tokens.rotated`) carries
`by` and, for an agent, `for`.

Membership is cell state: `fragment.json`'s `visibility`, `editors` and
`viewers` grant nothing (a `manifest.ignored` event). Only the owner, or
their agent for them, manages sharing; a member may leave. Who may do what
through each door (a browser, an agent, a job, the CLI, `fragment mcp`) is
cli/GUIDE.md's "Who can do what, where".

## Identities

The registry (`cell/src/registry.rs`; decisions 45 to 50) holds
identities, the keys each has held, each agent's owner, and sign-in
subjects with their verified emails; no grant. An identity is named for
good by the npub of the key it was made with: a person's, a key the
registry makes at their first sign-in and keeps sealed (it never signs
with it yet); an agent's, its agent fragment's own key. A retired key
still names its identity. A **key proof** is a NIP-98 event by the new key
for the same method and URL as the request carrying it, with `["p", <the
signer's key, 64 hex>]`.

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/identities` | a person | `{kind: "agent", proof}` → a new agent identity they own, named by the proof's key; again, the same one. A key someone else holds 409; an agent making one 403; `kind: "person"` 400 (people come from sign-in) |
| `GET /api/identities/{id\|me}` | the identity, or its owner | → `{id, kind, owner?, email?, picture?, createdAt, keys: [{npub, addedAt, addedBy, revokedAt?}], agents, subjects: [{issuer, email, linkedAt}]}`; anyone else 404 |
| `POST /api/identities/{id\|me}/keys` | a person for themselves; an owner for their agent | `{proof}` → the identity, the key added (at most `KEYS_PER_IDENTITY_MAX`, revoked ones counted); a person is mailed (Mail). Held by someone else, or revoked: 409 |
| `DELETE /api/identities/{id\|me}/keys/{npub}` | the same | → the identity; the key is 401 from the next request, for good. An agent's last active key 400 |
| `GET /api/identities/{id}/keys/{npub}` | the identity, or an agent it owns | → `{active}` |
| `PUT /api/identities/{agent}/held` | its owner | `{held: "viewer" \| "editor" \| null}` → the agent, held below its owner (decision 36): it acts with at most that wherever its owner's access reaches; its own memberships are not held. Anyone else 403; `owner` or `public` 400 |
| `PUT /api/identities/me/picture` | a person | the image (PNG, JPEG, WebP or GIF by its bytes; at most `PICTURE_MAX_BYTES`) → `{sha, mime}` |
| `GET /api/identities/{npub}/picture` | anyone | the bytes, `public, max-age=300`; none, 404 |

### An agent's name and picture

An agent is shown by its agent fragment at live: its **name** is
`fragment.json`'s `meta.title` (1 to `TITLE_MAX_CHARS`), its **role**
`meta.role` (at most `ROLE_MAX_CHARS`, a tag beside its name; only an
agent's), its **picture** the fragment's `avatar.png` (PNG, JPEG, WebP or
GIF by its bytes, at most `PICTURE_MAX_BYTES`; `fragment_core::face`). A
title or role past its limit makes the manifest invalid. They are the one
source, kept nowhere else: the fragment's cell reads them into its face
(`{kind, title, role?, picture?}`) at each install of live, and every surface
reads that face (`GET /api/fragments`, `__people`, `job.people`, the share
sheet, `GET /api/access`, an export, a computer's runtime, the shell).
An agent with no title, or one of only spaces, has no name
(`fragment_core::face::title`): every reader shows its label instead
(`face::agent_name`). Its label (`juniper` of `juniper--k3x9`) stays its
`name` in a profile and in its computer's view, the word an `@` mention
uses, which no rename changes. Once its fragment is deleted, its profile
has neither `name` nor `fragment`, so a life made again under the name
never lends it a face.

**Changing them** is writing those files and deploying (`POST
/api/f/{agent}/files`, then `POST /api/f/{agent}/deploy`), by its owner or
by the agent itself; `fragment agent [--name N] [--picture FILE |
--no-picture]` does that. An `avatar.png` past its limit (413) or that is
no picture (400), or a title past its limit, is refused and nothing is
committed; one pushed through git is refused at its install, which keeps
the last good face and says why in `status.code.error`. A rename by its
owner (Settings, `fragment agent --name`) rewrites, in the same commit,
each whole-word mention of an old name in its `SOUL.md` as the new one
(`fragment_core::face::job_renamed`); a job past `face::JOB_RENAME_MAX_BYTES` is
left as it is.

**Served** at `GET /api/identities/{npub}/picture`, the agent fragment's
npub, public as a person's is: the bytes an install checked, at an address
naming them (`?v=`, the first 12 hex of their SHA-256).

## Names (docs/cloudflare-v1.md, decision 47)

There are no usernames. A fragment's **name** is one DNS label: its
**label**, then `--`, then `NAME_SUFFIX_BYTES` characters the platform draws
(`todo--k3x9`; lowercase letters and digits less `l`, `o`, `0`, `1`),
unique in the fleet and fixed for its life. A label is lowercase letters,
digits and single dashes, neither first nor last. It is served at
`<name>.<suffix>`, a branch's at `<name>--<branch>.<suffix>`; that host
label is at most `HOST_LABEL_MAX_BYTES`, so a label may have
`fragment_proto::label_room` bytes on this deployment, and
`LABEL_ROOM_MIN_BYTES` fit every one. A create past it is 400
`invalid_request`, saying how long a label may be here; nothing is cut to
fit (`fragment_core::names`).

A create names a `label` (the suffix drawn, at most three draws) or a
`name` in full (taken: 409). Every path names a fragment in full: a bare
label is 404, saying so; the CLI resolves a bare label among its caller's
fragments.

Its code.storage repo is the deployment's prefix, then `<name>--<12 hex>`,
the hex a digest of its owner's identity
(`fragment_core::codestorage::repo_name`): the same owner making a deleted
name again finds its repo, and no one else ever does.

**A public address.** An app or a brain may have one: `<label>.<suffix>`
(`<label>--<branch>.<suffix>` on a branch), a second origin of the same
fragment; its name, repo, members and visibility are unchanged.

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/f/{name}/address` | viewer | → `{address: {label, url} \| null, suffix, maxLength}` |
| `PUT /api/f/{name}/address` | the owner, or their agent for them | `{label}` → the same: first come, first served, the same claim again answered again. A chat, agent, skills or sessions fragment, or an invalid label, 400; taken, or the app has another, 409; past the overdraft 402 |

A claim follows a label's rules, may hold no `--` (generated names and
computer hosts do), and may not be the platform's host. The registry keeps
it bound to that life's fragment npub and never releases or reassigns it,
so an old origin's cookies and storage never reach a new app: a deleted
app's address is 404, even once its name is made again. Each routed
request checks that npub.

## Operators: the admin (docs/billing.md; decision 59)

An operator is a key the deployment's `operators` (`FRAGMENT_OPERATORS`)
lists, held by anyone or no one (its signature is taken without asking
the registry); an identity it lists, signed or through the shell's
session; or a person who holds a key it lists, through their session. An
operator is named by their identity, or a key no one holds by its npub (a
grant's `by`, the log's).

`GET /admin` (the operators' page) answers only an operator's platform
session; to anyone else it is the 404 a missing route is, word for word.
Its API decides again on every call. `fragment operator
people|person|pin|orgs|comp|uncomp|trials|health|log|credit|budget|payer`
calls the same.

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/admin/people?q=&after=` | operators | → `AdminPeople {people: [AdminPerson {npub, email, joinedAt, lastSignInAt, org, admin, seat: {id, kind, comped, good, sleeps}}], next}`: 100 a page by npub; `q`, the start of their email (any case; wildcards match only themselves) |
| `GET /api/admin/people/{npub}` | the same | → `{person, ledger: LedgerStatus, computer: ComputerView \| null, references: {running, next} \| null}`: the full image references its computer runs and its next start runs |
| `PUT /api/admin/people/{npub}/computer/image` | the same | `{image}` → `ComputerView`, pinned as its owner's `PUT /api/computers/{id}/image` pins it; logged (`pin`). An image the deployment lacks 400; no computer 404 |
| `GET /api/admin/orgs?after=` | the same | → `AdminOrgs {orgs: [AdminOrg {id, name, createdAt, status, customer, subscription, periodEnd, cancelAtEnd, admins, paidSeats, compedSeats, pending}], next}` |
| `GET /api/admin/health` | the same | → `AdminHealth {orgsPaying, orgsLapsed, seatsPaid, seatsComped, planPushesQueued, quantityPushesQueued, failing: [{queue, target, tries, due}], lastEventAt, oldestCopyAt}` |
| `GET /api/admin/log?before=` | the same | → `AdminLog {entries: [{n, at, operator, action, target, detail}], next}`, newest first: every comp, trial code, pin and ledger command; the newest 10,000 kept |
| `POST /api/admin/objects/{class}/wake` | the same | `{id}` → `{class, id, ready: true}`: starts the platform object `id` (as Cloudflare lists it) of `class` (Registry, Principal, Ledger, Fragment, Computer) as a call would, its schema made current, and answers as a deploy's probe; refused, its `schema_refused`. An id of another class 400. Not logged: it changes nothing a call would not |
| `POST /api/admin/trials/{id}/send` | the same | `{email}` → `{sent, to}`: mails the code and `/welcome?trial=<code>`; an ended code 400 |

Comps, orgs and trial codes are under Seats and orgs.

## Operators: wiping a person

The deployment's operators wipe a person: everything that is theirs or
their agents' is deleted, and their email and sign-in are freed, so their
next sign-in is a new person. **An operator key** is one no person holds
(`fragment operator key <file>` makes one; its npub goes in `operators`):
a wipe ends its target's keys and sessions, so it never removes the key
doing it. An operator known by identity wiping that identity is 400. Any
other signer, or a browser session, is 403; unsigned, 401.

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/people/{person}/wipe` | operators | → `WipeReport`, the dry run; changes nothing |
| `POST /api/people/{person}/wipe` | the same | `{confirm, steps?}` → `WipeReport`: the wipe, as far as one call goes (`fragment_core::wipe::CALL_BUDGET_MS`, or `steps` steps). `confirm` must be the identity the dry run answered (another 409, none 400). Called again, it goes on where it stopped |

`{person}` is an email (whom a sign-in verified it for) or an npub. An
agent is 400 (it goes with its owner); no one, 404 (a wiped identity
still answers, `wiped`). `WipeReport`
(`fragment_proto::wipe`) is `{identity, email, state: live | wiping |
wiped, next, found, ran, done}`: `found` what they and their agents hold
now, `ran` this call's steps (`{step, done, deleted, note?, cleaning?}`).

**The steps** (`fragment_core::wipe::STEPS`, cell/src/wipe.rs), each
idempotent and recorded done only once whole:

- *Begin*, in one registry turn: the person is locked, their and their
  agents' sessions and keys end. Until the last step their sign-in is 403,
  nothing acts as them, and no one adds them to a fragment (404).
- `computer`: its container destroyed, its saves deleted from R2, its
  record emptied.
- `fragments`: each fragment they own ends as a delete ends it.
- `memberships`: they and their agents leave every other fragment, with
  their subscriptions and pushes there; what they wrote there stays.
- `cleanup`: until each ended fragment's cleanup is done (its members'
  lists, its app's database and blobs, its repo). `cleaning` names those
  left, `{fragment, lists, stored, repo, tries, held, error?}`; a part
  refused (4xx), or failing ten tries, is held and tried daily and at each
  call.
- `ledger`: emptied, keeping one row saying so; it refuses everything
  after (404).
- `lists`: their export, then their and their agents' lists, emptied.
- `pictures`: their picture, unless another identity set the same.
- `billing`: their place in their org; an org left with no one or no admin
  ends; Stripe cancels what they pay for (docs/billing.md, "A wipe").
- `registry`: every row naming them or their agents. The wipe's own row
  stays, and so do orgs' governance logs naming them by npub (never by
  email): each org they were in the content of (Org content) records them
  an admin no more, their waiting offers withdrawn, by the operator.

A wipe leaves what is not the platform's to delete: their WorkOS user and
its Pipes connections, a container snapshot until Cloudflare expires it,
Workflow history and logs for their retention, code.storage's soft-deleted
repos, their org's Stripe customer and invoices, what they wrote in
others' fragments, and a push they subscribed to on a public fragment
until its service says it is gone.

## Export

A person downloads everything that is theirs as one file (Settings' Your
data, `fragment export`). Only the person, as themselves, starts, reads or
downloads theirs: no agent, no operator, no lever (cell/src/export.rs).

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/export` | the person: the shell's session or their own key (an agent 403, `for` 400, unsigned 401) | no body or `{}` → `ExportView`: a new export, building; while one builds, that one. Past `export::limits::PER_DAY` in 24 hours, 429 `rate_limited`, saying when. A new one expires the last |
| `GET /api/export` | the same | → `{export: ExportView \| null}`, their newest |
| `GET`, `HEAD /api/export/{id}/download` | the person: signed, the shell's fetch, or a navigation the platform's own page started (`Sec-Fetch-Site: same-origin`) with their session | → the archive, `application/x-tar`, `attachment; filename="fragment-export-<YYYY-MM-DD>.tar"`, `private, no-store`; `Range` answers 206. Not theirs, not ready, or gone 404; another page carrying their cookie 401 |

`ExportView` (`fragment_proto::export`) is `{id, state: building | ready |
failed | expired, startedAt, progress: {done, total, bytes}, size?,
readyAt?, expiresAt?, error?: {code, message}, download?}`, `code` one of
`too_large`, `too_many`, `unavailable`, `source_changed`, `timed_out`.

It reads as its person, through the routes their own requests take (each
fragment decides their role and meters the request to its owner): who
they are, their and their agents' lists, each fragment they or their
agents own (a fragment shared with them is only named, `sharedWithMe`),
and their computer's newest save. What a fragment refuses or lacks is
named in its `meta.json` (`skipped`). The archive is one uncompressed tar
whose layout and formats its own README states
(`fragment_core::export::README`); its bounds are
`fragment_core::export::limits`, each a typed failure. Once ready its
person is mailed a link to `/settings`, and it is kept
`export::limits::KEPT_MS`. It is metered to its person (docs/ledger.md).

## Sign-in

A verified `(issuer, subject)` signs a person in: the issuer
`workos:<client id>`, the subject WorkOS's user id. The first makes the
person (Identities). WorkOS must have verified the email (403); it names
at most one person, so a sign-in whose email is another's is 409, never
merged. No route links a second sign-in to a person: AuthKit answers one
begun in a signed-in browser with the session it holds there. Browser
sessions live in the registry, looked up live whenever an answer depends
on who asks; a session lasts 30 days, its token 32 random bytes of which
the registry keeps the SHA-256.

| method & path (platform origin) | what |
| --- | --- |
| `GET /auth/login?return=&login_hint=&screen_hint=` | → WorkOS's authorize URL (`provider=authkit`, `redirect_uri` `<platform>/auth/callback`); `screen_hint` `sign-up` or `sign-in` (others ignored); its state bound by `fragment_login` |
| `GET /auth/callback?code=&state=` | the state must be the browser's cookie's (400); the code exchanged server-side; → `fragment_session` and back to `return`. A WorkOS `error`, a sign-in finished or past ten minutes, or a code WorkOS refuses: 400. A registry of another release refuses it unspent (503 `deploying`), so the same callback again signs in |
| `POST /auth/logout` | ends the session and every fragment session made from it, clears the cookie, and answers a page (200) that goes on to WorkOS's logout (`session_id` its `sid`, `return_to` `<platform>/`). No session: home. Another origin: 403 |
| `POST /auth/switch?return=` | "Use another account": ends this browser's session, then signs in as `/auth/login`, bound by `fragment_switch`. Where WorkOS answers from a session it already held (`auth_time` before the switch, or absent), no one is signed in and the browser goes through WorkOS's logout, so the next sign-in asks. Another origin: 403 |
| `GET /auth/fragment?name=&return=` | signed in: → `<fragment origin>/__signin?token=<redemption>` (single-use, 60 s, that fragment only; a session holds at most `REDEMPTIONS_PER_SESSION_MAX` unspent), at once for a fragment of theirs, shared with them, or one they said yes to; for any other, a page asking first (Asking first). Signed out: sign-in first |
| `POST /auth/fragment?name=&return=` | that page's form (`form`): the yes, remembered, → the fragment's `__signin?token=` (303). Another origin, or a missing or stale token, 403 |
| `GET /auth/frame?name=&return=` | a frame of the platform's own page signs in to the fragment (Frame sessions) |
| `POST /cli/begin` | `fragment login`'s new key, signing this itself (unsigned or stale 401; a key someone holds, or a revoked one, 409) → `{code, url, expiresAt}`: eight letters of RFC 8628's base-20, shown `WDJB-MJHT`, for ten minutes and once, `url` the plain `<platform>/cli`. The registry keeps its SHA-256 with the request's time, city and country, and `User-Agent`; a key keeps its newest `cli_code::CODES_PER_KEY_MAX`. Past `FRAGMENT_CLI_BEGINS_PER_IP_MINUTE` for its `CF-Connecting-IP`, or `CLI_BEGINS_PER_MINUTE_MAX` fleet-wide, 429 |
| `GET /cli` | signed in: a page with one field, for the code; it reads nothing from its address. Signed out: sign-in first |
| `POST /cli` | that form (`form`, `code`, any case, its hyphen or spaces optional): a page naming the email it will act as, the key's last eight characters, where, when and by what the code was asked, "Never type a code someone sent you", and Add. A code no CLI waits on is the page again (400) and a miss: a person's tenth miss in ten minutes makes every code they type 429 until ten minutes after their first. Another origin, or a missing, stale or too-soon token, 403 |
| `POST /cli/approve` | Add (`form`, `code`): the code spent (and its key's others), the key added to the signed-in person, who is mailed (Mail). A spent code 400; a key someone else holds, or a revoked one, 409; another origin or session's token 403 |

**Round trips** (`fragment_core::roundtrip`): every round trip that leaves
the platform and comes back (sign-in and switch at WorkOS, an own key's
sign-in at its provider: Connections) finishes only in the browser that
began it. Its state, 32 random bytes, is kept server side and bound to the
browser by a cookie of its kind (`fragment_login`, `fragment_switch`,
`fragment_own_key`, `fragment_connection`: HttpOnly, SameSite=Lax,
`Path=/`, ten minutes); a callback whose state is not that cookie's is
400. A second round trip of a kind replaces the first. In the cell only an
`auth::Bound`, made by `auth::trip_bound` from the request's own cookie,
finishes one (xtask's `a_round_trip_is_bound_only_by_its_request`).

**The platform's pages** answer `Content-Security-Policy: frame-ancestors
'none'` and `X-Frame-Options: DENY`, but for the share sheet and
`/auth/frame`'s notes, which only the platform's own pages may frame
(`frame-ancestors 'self'`, `SAMEORIGIN`); and `Cross-Origin-Opener-Policy:
same-origin`. Every form on them is 403 from another origin, a fragment's
page included; a POST without `Origin` is no browser's. The logout and
switch POSTs answer a non-cacheable page with a `Refresh` header and
`Referrer-Policy: no-referrer` rather than a redirect, so the shell's
`form-action 'self'` does not block the way to WorkOS.

Over https every session cookie with `Path=/` is named `__Host-…`
(`__Host-fragment_session`, `__Host-fragment_login`,
`__Host-fragment_switch`, `__Host-fragment_own_key`,
`__Host-fragment_site`) and read under that name only, so no fragment's
page can set one for another origin.

**On a fragment's origin**, `GET __signin?token=` redeems a redemption for
that fragment only (another's is 401 and stays unspent) and sets
`fragment_site` (HttpOnly, SameSite=Lax, host-only, `Path=/`). A frame
redemption is redeemed only in a frame, any other only outside one: shown
to the other kind it is 401, and spent. Without a token, `__signin` starts
at the platform (straight back if this origin's session is live), only as
a page's navigation: from an image, script or fetch it is 403, and in a
frame it answers a page that posts `{fragment: "signin-blocked", name}` to
its parent. A cookie for another fragment, or whose platform session
ended, is nobody. A platform session keeps its newest
`SITE_SESSIONS_PER_FRAGMENT_MAX` top-level and frame sessions on each
fragment. `POST __signout`, from the fragment's own page only (403
otherwise), ends the sessions its cookies carry, forgets the person's yes
to the fragment, and clears both cookies whatever the registry answers.

### Which cookies count

Every fragment's origin is one site with the others (the suffix is on no
Public Suffix List), so a SameSite=Lax cookie rides on another fragment's
requests. The router counts a browser's cookies on a fragment's origin by
the Fetch Metadata it sends, which no page's script sets; a request whose
cookies do not count is served as a stranger's:

| the request | `fragment_site`, `fragview`, `fragment_anon` | `fragment_frame` |
| --- | --- | --- |
| the fragment's own page's (`same-origin`) | count | count |
| a top-level navigation (`navigate`, `document`), GET or HEAD | count | no |
| a frame's navigation (`iframe`, `object`, `embed`), GET or HEAD | no | count |
| anything else: another page's image, script, fetch, form, any POST | no | no |
| a socket | count when its `Origin` is the fragment's own | the same |
| no Fetch Metadata (an old browser, or none) | count | no |

Every navigation's answer carries `Vary: Sec-Fetch-Dest`. A frame's
navigation is answered `Cache-Control: private, no-cache` and
`frame-ancestors`: signed in by a frame session, `'self' <platform>`;
from the fragment's own page, `'self'`; any other frame (a stranger's
answer), `'self' <platform>`. A signed request carries no cookies and is
unchanged.

### Frame sessions

A frame session is a fragment's session in a frame of the platform's own
page (the shell's windows: decision 6), bound to the platform's origin. A
cross-site frame gets no SameSite=Lax cookie, Safari blocks third-party
cookies, and a page's own code must never hold a sign-in token, so the
platform mints it for that frame alone and the frame keeps it in a
partitioned cookie (`fragment_core::frames`):

- `GET <platform>/auth/frame?name=&return=` mints only for a frame's
  navigation (`Sec-Fetch-Dest: iframe` or `frame`, `Sec-Fetch-Mode:
  navigate`) that a page on the platform's origin started
  (`Sec-Fetch-Site: same-origin`); anything else is 403.
- Where `/auth/fragment` would redeem at once, it answers a frame
  redemption (single-use, 60 s, its embedder the platform's origin) and
  `302` to the fragment's `__signin?token=` (`no-store`, `no-referrer`).
  Otherwise a note in the frame ("Open X in a tab", or "Sign in"), which
  posts `{fragment: "signin-blocked", name, why: "consent" |
  "signed-out"}` to the platform's origin alone.
- The fragment redeems it only in a frame's navigation, as
  `fragment_frame` (`HttpOnly; Secure; SameSite=None; Partitioned`,
  `__Host-` over https), then checks it was kept
  (`__signin?check=frame&return=`): without it, a page that posts
  `{fragment: "signin-blocked", name, why: "cookies"}` to its parent.
- `fragment_frame` counts only in frames and on the fragment's own page.

### Asking first

Signing in on a fragment is silent on the person's own fragments and those
shared with them (they or an agent of theirs is a member). Any other
fragment learns who they are only once they say yes on the platform's page
(unframed, a form token, a button armed after 800 ms); until then they are
a visitor there. The yes is kept per person and fragment (their newest
1000) until they sign out of it there (`POST __signout`).

### Opening a fragment by its URL (docs/cloudflare-v1.md, R4)

A top-level GET or HEAD navigation whose `Accept` names `text/html`, that
no session admits (401), goes to `<platform>/auth/fragment?name=&return=`
(`302`, `no-store`). Any other platform refusal a browser navigates to on a
fragment's origin is a small page with the refusal's status ("You need to
sign in", "You don't have access to X", "This link has changed"); an API
call, fetch, operation, socket, or a request not asking for HTML keeps the
JSON error. An app's own answers pass as they are (the fragment marks its
refusals with `x-fragment-refusal`, dropped before any answer leaves).

`return` is kept only when it begins with one `/`, holds no byte at or
below 0x20, no DEL and no backslash, and, percent-decoded once more, still
does not begin with `//`, `/\` or `/` and a control byte; anything else
returns to `/`. The way back is always absolute on its origin. A pending
sign-in lasts ten minutes and is kept through the next
`FRAGMENT_SIGNINS_PENDING_MAX` starts; expired sign-ins, redemptions and
sessions go on the registry's alarm.

## Sharing

The share sheet is a platform page, never a fragment's: a fragment's page
is its author's code, and sharing grants. It acts through the fragment's
own routes (members, invites, visibility, rotate) as the person signed in
on the platform, so the fragment decides who may do what.

| method & path (platform origin) | what |
| --- | --- |
| `GET /share/<name>` | the sheet, to any member (anyone else a 403 page): who is in and their roles (a member sees members' emails, decision 48; an agent by its title and its owner's email; a person with their agents as one entry, never naming them: decisions 63 and 64), the invites waiting, and General access; for the owner, the controls. It reads nothing from its URL. Signed out: sign-in first |
| `POST /share/<name>` | its form: `form`, `action` and its fields: `invite` (`email`, `role`), `role` (`member`, `role`), `remove` (`member`), `uninvite` (`email`), `visibility` (`visibility`), `rotate` (the share link only), `leave`. Done: 303 back to the sheet; refused by the fragment: the sheet saying why, with its status |

The sheet cannot be driven by a fragment's page. Every POST's `Origin`
must be the platform's (403), and carries `form`, an HMAC keyed by the
session's own token of what the form does (`share:<name>`) and when its
page was made (`fragment_core::form`): missing, another session's, another
form's, sent within 800 ms of its page, or older than 12 hours, it is 403.
Its buttons arm 800 ms after it shows. It sends no CORS headers, is framed
only by the platform's own pages, severs its opener, loads scripts,
styles and images only from the platform (`Content-Security-Policy`), and
sends `Referrer-Policy: same-origin`.

On a chat one of the owner's agents is in, the sheet, and `fragment
members add`, say "People in this chat can talk to your agent."
(`fragment_core::access::TRUST_WARNING`): whoever is in a chat may talk to
its agents, which act for their owner.

### Invites by email

Sharing is by email (decision 48), and an invite waits for its person's
yes (decision 63). The owner, or their agent for them, invites an email at
`viewer` or `editor` (`POST /api/f/<name>/invites`):

- Someone in it already is 400 `invalid_request`, saying as what; a role
  changes only with `PUT members`.
- Anyone else's invite waits `INVITE_TTL_S` in the fragment; the registry
  keeps it against the email and mails a link to the platform's page
  (never the fragment's). It answers `{invited}`. (`{member}` was an
  older answer: clients still read both.)
- Whoever signs in as the email sees it (`GET /api/invites`) and accepts,
  becoming a member at its role (a stronger one they hold is kept), or
  declines (`invite.declined`). Their agents come in with them, each a
  member as itself at their role, capped (R17; decision 64); one they make
  later comes in too. Their role is their agents' (an agent's own is 400),
  and removing or leaving takes them all out. An accept the fragment
  cannot take (at `MEMBERS_MAX`, or unreachable) keeps the invite. Only
  someone signed in as the email answers it; signing in never makes or
  meets a share. (Invites mailed before decision 63 are still met at
  sign-in until they run out: debt ledger, "Invites from before their
  yes".)
- An invite runs out at its expiry. Inviting the same email again renews
  it and mails it again; `DELETE invites/<email>` takes it back.
- A fragment holds at most `INVITES_MAX` waiting, and an email
  `INVITES_PER_EMAIL_MAX` across the fleet (429). A person mails at most
  `INVITE_MAILS_DAILY_MAX` a day, their agents' counted in (429). A mail
  the service does not send takes the invite back.
- No event names the email: `invite.created` and `invite.revoked` say a
  role and who made them, `invite.declined` no one, `member.joined` the
  npub that accepted.

Adding someone directly (`PUT members`) works only within one person: the
owner, or their agent for them, adds the owner's own agents. Anyone else
is 403, saying to invite them by email.

A person answers their own invites, with their session or a key; or
their own agent does, `for` them and not held (its yes makes its owner a
member). Any other agent is 403; a person naming `for` 400:

| method & path | body → answer |
| --- | --- |
| `GET /api/invites` | → `InvitesWaiting {invites: [{fragment, role, title, from, email, expiresAt}]}`: those waiting on their emails, soonest to run out first; `title` the fragment's as sent, `from` the sharer's email (an agent's: its owner's; null when no one signs in as them now) |
| `POST /api/invites/{fragment}/accept` | `{}` or none (else 400) → `InviteAccepted {fragment, role}`, the fragment in their list before it answers. None waiting: 404 (`reason: gone`); the fragment's refusal keeps it |
| `POST /api/invites/{fragment}/decline` | the same → `{ok, declined}`; none waiting: 404 (`reason: gone`) |

Their pages are told when the invites waiting on them change: `{type:
"invited"}` on `GET /api/fragments/watch`.

### Who has access to what

`GET /api/access` (a person, signed or with the shell's session; an agent
403, `for` 400) → `Access {fragments: [{name, kind, title?, sharing?,
agents?, members?, more?, invites?}]}`: each fragment they own, in their
list's order, `sharing` and `agents` as their list has them. A fragment
someone else may be in or invited to is asked, as the person, for its
members and invites: its first `ACCESS_ASKED_MAX`, `ACCESS_ASKED_AT_ONCE`
at a time. `members` is everyone but the owner (at most
`ACCESS_MEMBERS_MAX`; `more` counts the rest), each `{principal, role,
kind, email?, name?, title?, owner?, ownerEmail?}` (a person by email; an
agent by its label, title and owner's email), and `invites` as `GET
invites` answers them. At most `ACCESS_PROFILES_MAX` identities are named
by email. A fragment that does not answer keeps its list's counts.

## Control API

Every route here is signed (or the shell's session: The shell), but the
inbox's, which takes its token. `{name}` is a fragment's name in full.

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/fragments` | a person who is not a guest; an agent for its owner (the owner's, billed to them, its maker an editor) | `{label \| name, visibility?, template?, title?}` (Names) → `{name, npub, owner, visibility, viewToken, inboxToken, repo, canonical}`. A guest is 403 `forbidden` (`Spend::Create`), however asked, and past the overdraft 402 `budget_used_up`; a ledger that does not answer refuses none. `visibility` defaults to `link`, but `members` for a chat, an agent or a session archive (`Visibility::made_from`). The fragment's own key is made in its cell, sealed. With `template` (`blank`, `todo`, `inbox`, `calories`, `doc`; any other is 400 and nothing is made), the template's files are main's first commit (its `fragment.json` stamped with the name, and `title`) and live once the create answers; one that fails to land is retried by the alarm (`template.failed`). `agent`, `chat`, `brain`, `skills`, `sessions`, `telegram` are blessed (decision 40): main's first commit is `{"template", "meta": {title}}`, and the release serves the rest (Apps; a session archive, which `fragment session import` makes: templates/sessions/README.md). `notes` is the CLI's only. A `title` (1 to `TITLE_MAX_CHARS`) needs a `template` (400) |
| `GET /api/fragments` | any signer | → `{fragments: [ListedFragment {name, role, kind, title?, picture?, agentRole?, agents?, preview?, previewAt?, unread?, sharing?, archived?, dock?, pinned?, listed?}]}`. `kind` is `app`, `chat`, `agent`, `brain`, `skills` or `sessions` (a client reads an unknown one as `other`: `FragmentKind::Other`); `picture` an agent's face or an app's icon; `agentRole` an agent's role (An agent's name and picture); `agents` its agent members, the first added first, at most `LISTED_AGENTS_MAX`; `preview` (at most `LISTED_PREVIEW_MAX_BYTES`) and `previewAt`, a chat's newest message their search holds (Search); `unread`, a chat's messages after their last `seen`, at most `LISTED_UNREAD_MAX`, absent at 0; `sharing`, their own fragments' `{visibility, members, guests, invites}`; `archived: true`, `dock: false` and `pinned: true` their own choices; `listed`, what its members are doing (Listed records). An agent's `?for=<id>`: the fragments that identity holds a role on where the agent or its owner is a member, each with the role the agent acts with there for it and `owned: true` on that identity's own, without `listed` |
| `GET /api/fragments/watch` | any signer; the shell's session with the platform's exact `Origin` | a WebSocket: `{type: "hello"}`, then `{type: "changed"}` whenever their list changes, and `{type: "invited"}` whenever the invites waiting on them change. Frames name nothing: the page reads again with its own credential. A fragment's page is refused like no one (401). At most `LIST_WATCHERS_MAX` a list (429). Not for `for` |
| `PUT /api/fragments/{name}/archived` | any signer with a role on it | `{archived: bool}` → `{name, archived}`: their own view, in their list's row only. It goes when they leave, or the fragment is made again. No role, none, or a bare label 404; a bad body 400. Not for `for` |
| `PUT /api/fragments/{name}/dock` | the same | `{dock: bool}` → `Docked {name, dock}`: whether their dock shows it, as archiving is (every fragment is on it until taken off); their list's sockets are told. Not for `for` |
| `PUT /api/fragments/{name}/pinned` | the same | `{pinned: bool}` → `Pinned {name, pinned}`: whether they pinned it, as archiving is (nothing is pinned until pinned); their list's sockets are told. It goes when they leave, or the fragment is made again. Not for `for` |
| `PUT /api/fragments/{name}/seen` | the same | no body → `Seen {name, unread: 0}`: they have seen its messages up to the newest their search holds; it never goes back. Any body 400. Not for `for` |
| `GET /api/search?q=` | any signer | → `SearchAnswer {fragments: [ListedFragment], messages: [{fragment, channel, seq, at, snippet}]}` (Search). `q` once, at most `SEARCH_QUERY_MAX_BYTES` and `SEARCH_QUERY_WORDS_MAX` words (400). Not for `for` |
| `POST /api/search` | any signer, `for` too | `Across {q, in, limit?, mode?, fields?, after?, before?}` → `FoundAcross {results, fragments, semantic}`: one question asked of several fragments' indexes (Search across fragments) |
| `GET /api/access` | a person | Sharing, "Who has access to what" |
| `DELETE /api/f/{name}` | the owner (never an agent) | → `{ok, deleted}` once it is gone: 404 to everyone, out of its owner's list, its name free. Its members' lists, app database and blobs go after, by its alarm (`fragment_core::ended`: retried, a part refused or failing ten tries held and tried daily). An agent fragment's agent leaves its computer and is named by it no more. A life made again under the name is untouched by the old one's cleanup. The repo stays |
| `GET /api/f/{name}/status` | viewer | → `{name, kind, title?, agentRole?, npub, owner, role, visibility, repo, pins: {main, live}, counts: {files, events, members}, code: {sha, id, operations: {<op>: {kind, role, input?, ephemeral?, description?}}, error}, viewToken, inboxToken, urls: {canonical, platform}, blobMinBytes, page}`: `viewToken` the owner's (or their agent's for them, but on a connector), `inboxToken` an editor's; `code.id` `app:<hash>` of `app.mjs` and `applib/`, or `blessed:<template>@<release>`; `urls.platform` the platform's origin, for links a person opens; `page` the card's report (Cards), absent before the first |
| `GET /api/f/{name}/manifest` | viewer | → `fragment.json` at main; none 404 |
| `GET /api/f/{name}/members` | viewer | → `{members: [{principal, role, addedBy, addedAt, kind, owner?}]}` |
| `PUT /api/f/{name}/members/{id\|npub\|email}` | owner, or their agent for them | `{role: viewer \| editor}` (another field 400) → the member, added or its role changed. A new member this way is one of the owner's own agents alone; anyone else 403 (invite them). An email no one signs in as, 404. A person's agents follow their role (an agent's own 400). A chat's lead agent (the first added) is never set below `editor` (400). An agent on a computer is told (`joined` on its `tasks`) and its computer woken |
| `DELETE /api/f/{name}/members/{id\|npub\|email\|me}` | owner, or their agent for them; the member | → `{ok, removed}`, closing their feeds. A person goes with their agents in it, and such an agent takes its person and their other agents (decision 64). A chat's lead agent is removed by no one else (400); it may leave |
| `POST /api/f/{name}/invites` | owner, or their agent for them | `{email, role}` → `{invited: {email, role, expiresAt, createdBy}}` (Invites by email). In it already 400 |
| `GET /api/f/{name}/invites` | the same | → `{invites: [{email, role, expiresAt, createdBy}]}`, oldest first |
| `DELETE /api/f/{name}/invites/{email}` | the same | → `{ok, revoked}`; none 404 |
| `PUT /api/f/{name}/visibility` | the same | `{visibility}` → `{ok, visibility}`. Leaving `members` makes a new share link |
| `POST /api/f/{name}/rotate` | the same | `{scopes?: [inbox, view]}` → `Rotated {inboxToken, viewToken, rotated}`; a new view token closes link holders' feeds |
| `PUT /api/f/{name}/cap` | the owner (never an agent) | Ledger |
| `PUT /api/f/{name}/secrets/{KEY}` | editor | `PutSecret {value, hosts}` (at most `SECRET_MAX_BYTES`; 1 to `SECRET_HOSTS_MAX` hosts, each a bare host name: no scheme, port, path or wildcard) → `{ok, name, hosts}`, sealed per Durable Object (docs/secrets.md). `job.fetch` fills it only toward one of its hosts, over https, in a request that sets no `Host`, `Forwarded` or `X-Forwarded-Host` (`fragment_core::secrets::may_fill`). Hosts change only with the value. No event or refused fetch names a host |
| `GET /api/f/{name}/secrets` | editor | → `SecretList {secrets: [{name, hosts}]}`; values never leave |
| `DELETE /api/f/{name}/secrets/{KEY}` | editor | → `{ok, removed}` |
| `GET /api/f/{name}/door` | its owner | → `{bound: {door, agent} \| null}`: whose door this connector is (An agent's doors) |
| `POST /api/f/{name}/door` | its owner, in person (never an agent) | `{act: "link", agent}` → `{door, agent, relayed}`, or `{act: "unlink"}` → `{ok}` (An agent's doors). No connector, one the deployment does not allow, or an agent not the owner's, 403; another `act` 400 |
| `GET /api/f/{name}/storage-token` | editor | → `{token, repo, api, expiresAt}`: ES256, this repo, `git:read`+`git:write`, `STORAGE_TOKEN_TTL_S` |
| `POST /api/f/{name}/refresh` | editor | → `{ok, refs: {main: {pin, moved} \| {absent}, live: …}}` once a moved live is installed. A writer to the repo asks this after its push; the platform follows its own moves, and a busy fragment's poll is the backstop (`FRAGMENT_POLL_INTERVAL_S`) |
| `POST /api/f/{name}/files` | editor | `{files: [{path, text \| base64} \| {path, delete: true}], message?, key?}` → `{commit}`: one commit to main (at most `FILE_WRITES_MAX` files, 400; `API_FILES_WRITE_MAX_BYTES` decoded in a body of `BODY_MAX_BYTES`, 413; relative paths, no `.` or `..`). The same `key` from the same person answers the first commit again; a write of what main holds commits nothing. Main moves at once; live does not |
| `POST /api/f/{name}/deploy` | editor | `{note?}` → `{live, canonical}`: live to main's tip, guarded against a live that moved meanwhile; the app installs at once |
| `GET /api/f/{name}/files` | viewer | → `{ref, files: [{path, size, mode, lastCommitSha, machinery, blob?, sha256?, release?}]}` at main; a pointer's `sha256` is its blob. A blessed template's data from the release is listed beneath (`release: true`) |
| `GET /api/f/{name}/file?path=` | viewer | → the bytes at main (`x-fragment-ref`); with none of its own there, a blessed template's data file |
| `PUT /api/f/{name}/blobs/{sha256}` | editor | the bytes (`content-length` required, at most `BLOB_MAX_BYTES`), hashed on the way → `{ok, sha, size, stored}`; other bytes 400. Its `content-type` is what `__blob` serves when it is passive media (Blobs) |
| `GET`, `HEAD /api/f/{name}/blobs/{sha256}` | viewer | → the bytes (ranges 206) |
| `GET /api/f/{name}/card` | viewer | → the preview card: a 1280×800 JPEG, `ETag` its SHA-256, `private, no-cache`, `X-Fragment-Ref` the live it shows (an upload has none); `If-None-Match` 304; none yet 404 (Cards) |
| `POST /api/f/{name}/card` | editor | → `{ok, live, since, joined}`: a shot of the live again (Cards). A chat, agent or members-only fragment, or nothing deployed, 400; an owner who pays for nothing 402; within `card::AGAIN_EVERY_MS` of the last ask, 429 |
| `PUT /api/f/{name}/card` | editor | a 1280×800 JPEG of at most `card::BYTES_MAX`, whole (`media::jpeg_whole`) → `{ok, blob, size}`. Anything else, or a chat or agent, 400; too big 413; past the overdraft 402 |
| `GET /api/f/{name}/icon`, `PUT`, `DELETE`, `POST …/icon/make` | Icons | |
| `GET /api/f/{name}/events?since=` or `?tail=` | viewer | → `{events: [{id, at, kind, summary, data}]}`, oldest first: after `since`, or the newest `tail` (1 to `EVENTS_PAGE`; else, or with `since`, 400). `AUDIT_KEPT` kept |
| `POST /api/f/{name}/ops/{op}` | the operation's role | `{id, input}` → `{result, replayed}`; a job's `result` is `{run, status}` |
| `GET /api/f/{name}/runs?status=&op=&limit=` | viewer | → `{runs: [{id, op, via, trigger, principal, status, attempt, depth, createdAt, finishedAt, error}], counts, paused}`, newest first (30, at most 200) |
| `GET /api/f/{name}/runs/{id}` | viewer | → one run with `input` and `output`. A channel's run whose channel the asker may not read (or no longer declared) withholds `input` and `error` (`withheld: true`). `output` is the app's own answer, shown to viewers whatever it copied |
| `POST /api/f/{name}/replay` | editor | `{run}` → `{ok, run, attempt}`: a `held` or `blocked` run again |
| `POST /api/f/{name}/pause` | editor | `{op, paused}` → `{ok, op, paused}`: its triggers stop or start; calls are never paused |
| `GET /api/f/{name}/triggers` | viewer | → `{triggers: [{cron \| channel \| files \| installed, run, paused, nextAt?}], paused}` |
| `GET /api/f/{name}/schedules` | viewer | → `{schedules: [Schedule]}`, oldest first (Scheduled records) |
| `POST /api/f/{name}/schedules` | editor holding the channel's post role | `{cron, channel, record}` → `{schedule, replayed}`; past the overdraft 402 |
| `PUT /api/f/{name}/schedules/{id}` | editor | `{paused}` → `{schedule}`; a resume past the overdraft 402 |
| `DELETE /api/f/{name}/schedules/{id}` | editor | → `{ok, removed}`; none 404 |
| `GET /api/f/{name}/database?table=&offset=` | the owner (never an agent) | its app's database, for an export (`__dump`, cell/platform.mjs). No `table`: `{schema: [{type, name, table, sql}], tables}`, the platform's own tables left out. A `table`: `{table, offset, rows, next}`, at most 1000 rows and about 4 MiB from `offset`, each a JSON object (a blob `{"$base64"}`); not in the schema 404. No app code: 404 `no_code` |
| `POST /api/f/{name}/inbox` | the inbox token, or the secret its code's `inbox` names (Apps) | the token in `x-fragment-inbox-token` or `?t=`, or, when its code names an inbox secret, that header holding it (and no token: 403 otherwise); JSON `{source?, payload}` (any other JSON, or text, is the payload), at most 64 KiB → `{ok, seq, runs}`. Bad token 403; `INBOX_PENDING_MAX` pending 429 |
| `POST /api/f/{name}/subscriptions` | a member who may read the channel | `{channel, url}` → `{id, channel, url}`: each new record POSTed to `url` as `{type: "record", fragment, channel, record}` (unsigned: the URL is the capability; egress-checked; at most 32 a fragment; a 404 or 410 drops it; a removed member's go). `{channel, wake: true}` in place of `url`, from a computer's egress alone (400 from anywhere else), wakes its computer instead (docs/computers.md) |
| `GET /api/f/{name}/subscriptions` | a member (the owner sees all) | → `{subscriptions: [{id, principal, channel, url, createdAt}]}` |
| `DELETE /api/f/{name}/subscriptions/{id}` | its subscriber, or the owner | → `{ok, removed}` |
| `GET /api/f/{name}/channels` | viewer | → `{channels: [{name, read, post, signedIn, from, listed, seq}]}`: `events`, `ops`, `inbox`, and the app's (Apps) |
| `GET /api/f/{name}/channels/{channel}?after=\|before=&limit=` | the channel's reader | → `{channel, records: [{channel, seq, at, principal, kind, body}], next}`, at most `CHANNEL_PAGE` a page, always ascending. `after` (default 0) reads forward; `before` the records just below it. `next` is the next page's cursor. Both cursors, or a bad cursor or limit, 400 |
| `POST /api/f/{name}/channels/{channel}` | the channel's `post` role | `{id, body}` → `Posted {record, replayed}`: `body` (any JSON, at most `RECORD_BODY_MAX_BYTES`; 413) appended as a `message` record naming the poster, with no app code, reaching sockets, subscriptions and triggers. The same id and body again answer that record (and finish what the first left); the same id otherwise 409. A channel without `post`, and `events`, `ops`, `inbox`, 403; `signedIn` refuses an anonymous poster (401); `from` refuses all but a member of that kind posting as itself (403). A `public` poster spends a public call |
| `PUT /api/f/{name}/channels/{channel}/draft` | the channel's `post` role | `{turn, text}` → `{ok}`: a record its poster is writing, whole so far (at most `RECORD_BODY_MAX_BYTES`), to the sockets following the channel (`draft` on `__live`), never stored; `text: null` stops it. At most 10 a second a fragment (429); `turn` is `^[A-Za-z0-9._:-]{1,128}$` |

### Search (decision 9; docs/cloudflare-v1.md, lesson 12)

Search is FTS5 in each person's `Principal` cell, fed by the fragments as
the list is (cell/src/search.rs, `fragment_core::search`).

- **What is searched.** A record is a message when its body is an object
  whose `kind` is absent or `"message"`; its text is the body's string
  `text`, its first `SEARCH_TEXT_MAX_BYTES`. Only channels every member may
  read (`viewer` or weaker) are searched, never `events`, `ops` or `inbox`.
  Titles and labels are searched from the list. Files are not.
- **Who gets it.** A fragment sends its messages to its members who are
  not agents, from an outbox, in batches; one who joins gets what it still
  logs.
- **The fence.** A list takes a fragment's messages only while its row for
  that fragment names a role, at the fragment's incarnation, each once.
  Leaving, a delete, or a channel tightened past `viewer` drops its
  entries. Search sees only what the person can see now.
- **Limits.** `SEARCH_ENTRIES_PER_FRAGMENT_MAX` a fragment and
  `SEARCH_ENTRIES_MAX` in all, oldest first out; an answer has at most
  `SEARCH_FRAGMENTS_MAX` fragments and `SEARCH_MESSAGES_MAX` messages, each
  with a plain-text snippet of at most `SEARCH_SNIPPET_MAX_BYTES`.
- **A query is words.** Each (split at spaces; one with no letter or digit
  dropped) must be in the message as a word or a word's start, ignoring
  case and accents. FTS5's syntax is never read.

A chat's newest entry is its row's `preview`.

### Listed records

A channel that declares `listed` (Apps) puts what each member is doing on
its members' rows (`GET /api/fragments`'s `listed`), so a page following a
person's list follows every fragment's people without polling one
(cell/src/listed.rs, `fragment_core::listed`). The platform never reads
what a kind means.

- **What is listed.** A record whose body's `kind` is a string the channel
  lists (a post's record kind is always `message`; a published record is
  listed only if its body names the kind too), as `ListedRecord {channel,
  principal, kind, fields?, at}`: its listed fields that are strings, each
  cut to `LISTED_VALUE_MAX_CHARS`. Only members' records, each member's
  newest a channel, its `LISTED_POSTERS_MAX` latest posters.
- **When rows go.** When a record shows something the last sent did not,
  or the same `LISTED_FRESH_MS` later; at most once in `LISTED_MIN_GAP_MS`
  a fragment. The person's pages are told (`watch`'s `changed`).
- **The fence.** Rows are made as sent, from what is installed then, only
  for channels the row's role may read; a role change, a removal, or a
  deploy changing `read` or `listed` sends them again. An agent's list
  read `for` someone carries none of theirs. History is never read back.

### A fragment's search

A fragment's own full-text and semantic index: a `SearchIndex` object of
each life's own (cell/src/search_index.rs), which its supervisor alone
calls (cell/src/searchable.rs). Shapes are `fragment_proto::search`'s, and
so are the limits named here.

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/f/{name}/search/upsert` | an editor | `Upsert {source, revision, replaces?, title?, at?, fields?, sections: [{id, text, heading?, context?, at?, location?: {from, to}}]}` → `{source, revision, written, chunks}` |
| `POST /api/f/{name}/search/delete` | an editor | `{source}` → `{source, deleted}` |
| `GET /api/f/{name}/search/sources?after=&limit=` | an editor | → `{sources: [{source, revision, at?, chunks}], next}`, by source, at most `SOURCES_PAGE_MAX` |
| `GET /api/f/{name}/search/status` | a viewer | → `{exists, sources, chunks, bytes, space, chunks_split, chunks_max, semantic, forgetting}` |
| `POST /api/f/{name}/search` | a viewer | `Query {q, limit?, mode?, fields?, after?, before?}` → `Found {results: [{rank, source, section, revision, title, heading?, context?, at?, location?, snippet, score, signals, lexical?: {rank, bm25}, semantic?: {rank, cosine}}], semantic, indexed, space?, scheme?}` |

- **One index a fragment's life**, which no caller names; whoever reads
  the fragment searches it. Nothing is held until an editor writes: until
  then a search answers `indexed: false` and no results, embedding
  nothing.
- **A source is written whole**, in one transaction: its sections replace
  what it held. The same `(source, revision)` again writes nothing;
  `replaces` is a compare-and-set on its revision (`""`: none yet), else
  409 `conflict` naming `current`.
- **Bounds**, refused 400 (413 past `UPSERT_MAX_BYTES`) before anything is
  sent: `SECTIONS_MAX`, `TEXT_MAX_CHARS`, `SECTION_ID_MAX_BYTES`,
  `SOURCE_MAX_BYTES`, `REVISION_MAX_BYTES`; at most 4 `fields`. An index
  holds at most `chunks_max` chunks (507 `storage_full` past it); past
  `chunks_split` a search slows: split the sources across child fragments.
- **Embedding** is the deployment's embeddings slot's (Models), each
  section cut into chunks the slot reads whole, a chunk the index holds
  never embedded again. A write is paid for by the fragment's owner, a
  question by its asker (an agent's owner). An anonymous question, and
  every one on a deployment that embeds nothing (`semantic: "off"`), is
  searched by its words alone. An index holds one `space`'s vectors: a
  write in another (the deployment's model changed) empties it first,
  and its apps write their sources again.
- **Search** is by words (BM25 by field: title, heading, context, text)
  and by meaning (vectors held as `scheme` names, `i8x508+sign`), each
  section ranked by its best chunk and the legs fused by rank: `mode`
  `hybrid` (the default), `lexical` or `semantic`; `fields` filters by
  each field named, `after` and `before` by a section's `at`. A hit's
  `lexical` and `semantic` are its rank and score in each leg; cosines of
  answers naming one `space` and `scheme` compare across fragments.
- **Many fragments at once**: the platform's own route embeds a question
  once and asks each fragment's supervisor at `POST /search/asked`
  (`Forwarded {query, vector?}`), which the router maps no outside path
  to. It is never embedded again, and answers as `POST …/search` (Search
  across fragments, below).
- **Deletes follow the files**: a source named by one of the fragment's
  paths is forgotten when a move of main deletes that file, gone from every
  answer at once. A deleted chunk's words leave the index's segments at
  its next fold (an hour after); SQLite's freed pages and Cloudflare's 30
  days of point-in-time recovery may keep its bytes that long. A deleted
  or wiped fragment's index ends with it: released whole, and never
  written again, by a write landing late too.


### Search across fragments

`POST /api/search` (cell/src/across.rs, merged by fragment_core::across):
one question asked of several fragments' indexes as one, as the signer: a
parent brain and its sources, or a person's brains and Session Archive.
`fragment search --in` and `fragment mcp`'s `fragment-search` tool ask it
(cli/GUIDE.md, "Searching fragments").

- **Each fragment decides alone.** Each named (1 to `IN_MAX`, in full,
  once) is asked at once, as the signer, exactly as on its own route:
  its role, its life, what it forgets. So nothing is answered that the
  signer could not have from each, and nothing is held or indexed.
  `fragments` says how each answered, in the order asked: `searched`,
  `no_index`, `denied`, `missing`, or `failed` with its `message`.
- **The question is embedded once**, paid by its asker (an agent's owner),
  and each fragment searches with that vector; `mode: lexical`, a
  deployment that embeds nothing, or a refused embedding: words alone
  (`semantic: "off"`).
- **The merge.** Each fragment is asked for twice the answer's length (at
  least `ASKED_MIN`, at most `RESULTS_MAX`). Cosines of answers naming the
  question's `space` and `scheme` compare, so semantic hits sort together
  by cosine; BM25 does not (each index's statistics are its own), so
  lexical hits interleave by each fragment's rank, the nearer meaning
  first among equals. The two fuse by reciprocal rank (k = 60). A hit's
  `rank` and `score` are the merged answer's, its `lexical` and `semantic`
  its fragment's, and `fragment` names it.
- The body is at most 16 KiB; the question is checked as a fragment's
  search checks it, before anything is embedded or asked.

## Apps

Code comes from git: when `live` moves, the cell reads `fragment.json` and
`app.mjs` (at most `SOURCE_MAX_BYTES`; with `applib/**.mjs|js`, at most
`APPLIB_FILES_MAX` modules and `APP_MODULES_MAX_BYTES` in all). A live
commit with an invalid `fragment.json` keeps the last good code and says
why in `status.code.error`.

A fragment on a blessed template (`fragment_templates::blessed::BLESSED`;
decision 40) names it in `fragment.json` (`template`) and runs the
release's manifest (its own `meta` over the template's), site and code
(held to an app's limits, run in the same facet, as
`blessed:<template>@<release>`), read from the release's files. A deploy of
the platform that changes the template installs again at each such
fragment's next request (`code.installed`). Its repo holds only its face
and data: a live commit declaring operations, channels, triggers or an
agent, or carrying code, keeps the last good code and says to fork it.

```json
{
  "operations": {
    "add":  { "kind": "mutation", "role": "editor", "description": "Add a todo with this text. Answers its {id}.",
              "input": { "type": "object", "required": ["text"],
              "properties": { "text": { "type": "string", "maxLength": 200 } }, "additionalProperties": false } },
    "list": { "kind": "query", "description": "Every todo, oldest first: {todos: [{id, text, done}]}." }
  },
  "channels": { "activity": { "read": "viewer" } },
  "meta": { "title": "Todo", "description": "…", "image": "https://…" }
}
```

- `kind` is `query`, `mutation` or `job`. `role` defaults to `viewer` for a
  query and `editor` for a mutation or a job. `RESERVED_OP_NAMES`
  (`constructor`, `fetch`, `alarm`) are refused at deploy.
- `input` is a JSON Schema in a bounded subset (`crates/core/src/schema.rs`;
  annotations allowed, any other keyword refused at deploy). A call whose
  input does not fit is 400 naming the JSON pointer (`input /text: is
  required`).
- `description` (1 to `OP_DESCRIPTION_MAX_CHARS` characters; line breaks
  and tabs, no other control character) says what the operation does and
  answers. Anything else is refused at deploy, naming the operation. A
  described operation is a `fragment mcp` tool.
- `ephemeral: true` (a mutation only; refused at deploy elsewhere): its
  calls leave no ledger row, pending row or `ops` record, so an id runs
  again with any input, and it may not publish, push or write files (422).
  For a latest-value write sent often.
- `channels` declares the app's channels and their `read` role (default
  `viewer`); `events`, `ops` and `inbox` are built in (viewers read them).
  At most `CHANNELS_MAX`. A channel may also say:
  - `"post": <role>` (R18): whoever holds it posts (`POST
    …/channels/{channel}`, `fragment.post`), appended by the platform with
    no app code. No looser than `read` (refused at deploy). It keeps its
    newest `POSTED_KEPT` records.
  - `"signedIn": true` (with `post`): only people signed in post (401
    `unauthenticated` otherwise).
  - `"from": "person" | "agent"` (with `post`): only members of that kind,
    posting as themselves (403 `forbidden` otherwise, drafts too); an app
    publishes nothing there.
  - `"listed": {"kinds": [...], "fields": [...]}` (Listed records): 1 to
    `LISTED_KINDS_MAX` kinds (`^[A-Za-z0-9._-]{1,64}$`) and at most
    `LISTED_FIELDS_MAX` fields (`^[A-Za-z][A-Za-z0-9_]{0,31}$`, never
    `kind`), each once; else refused at deploy.
- `triggers` (at most `TRIGGERS_MAX`) start runs of a mutation or a job an
  editor may call: `{"cron": "0 9 * * *", "run": op}` (five fields, UTC,
  1 = Sunday); `{"channel": "inbox" | <app channel>, "run": op, "from"?:
  "person" | "agent"}` (each new record, of that kind's members only with
  `from`); `{"files": "notes/**", "run": op}` (a move of `main` changing a
  matching path: `*`, `**`, `?`, a trailing `/`); `{"installed": true,
  "run": op}` (once per install of new code: a deploy moving `live`, or a
  blessed template's changed release, at the fragment's next request; its
  input `{ref: "live", commit, code}`).
- `inbox` (optional): `{"header": "x-telegram-bot-api-secret-token",
  "secret": "TELEGRAM_WEBHOOK_SECRET"}`: the inbox takes a delivery only
  with that header (lower case; never `x-fragment-*`, `cookie`, `host`,
  `authorization` or `content-type`) holding the fragment's secret of
  that name, compared in constant time; the inbox token then admits
  nothing, and nothing is taken before the secret is set. So a delivery
  is forged only by whoever may set that secret (its editors; on a
  connector, its owner alone), never by a reader of the inbox token. A
  fragment on a blessed template declares none.

`app.mjs` exports `class App extends DurableObject` with a method per
operation, called `(input, call)`: `call.principal` (an identity, `anon:…`,
or the fragment's own npub for its triggered runs; for an agent's call
`for` someone, them), `call.agent` (that agent, else null), `call.role`.

- A **mutation** is synchronous over the app's own SQLite.
  `call.publish(channel, body, kind = "message")` appends a record (at most
  `RECORD_BODY_MAX_BYTES`) once it commits; an exception rolls back its
  writes and records (422). Its records, file writes and pushes are its
  effects, at most `EFFECTS_MAX`. The ledger keys it by (principal, id) for
  seven days: the same id answers the stored result, with another input
  409. Every applied mutation appends `{op, id}` to `ops`. Its effects are
  checked in the app and again in the cell: a refusal there drops them all,
  answers 422 `app_failed` (its writes stand) and records
  `effects.refused`; a failure applying them answers 502 or 500 and the
  cell retries for about two days (`effects.delayed`,
  `effects.abandoned`).
- A query's or mutation's result is at most `RESULT_MAX_BYTES` of JSON (422
  in the app, 413 in the cell).
- An optional `fetch(request)` answers every path that is not a site file,
  with `x-fragment-principal` and `x-fragment-role` set; its redirects
  reach the browser as they are.
- The app's database holds at most `APP_DB_MAX_BYTES`: a mutation past it
  rolls back, 507 `storage_full`. A write from elsewhere (a query, `fetch`)
  is not stopped: the check is a courtesy in the app's code (debt ledger).
- Before the author's constructor runs, the platform takes away the app's
  alarm, async transactions, KV writes and facets (`cell/platform.mjs`): a
  call to one throws. The runtime refuses `eval`, `new Function` and
  `Atomics.wait`, and holds the app to its CPU and memory limits (422). At
  the facet's concurrency limit a call is 503 `node_full`.
- Each fragment's app runs in its own loaded worker: its env holds only its
  own capabilities, and no module state is shared with another fragment.

### Files

The app's files are the fragment's files in git. Reads come from `main`
at its pin; writes land on `main` as commits the cell makes.

- `this.files.read(path)` (text), `readBytes(path)`, `list(prefix)`
  (`[{path, size, blob?}]`), `stat(path)` (`{path, size, sha, commit}`)
  from anything async. A file over `FILE_READ_MAX_BYTES` (a blob) is 413;
  the site serves it. An absent file is `null`.
- In a mutation, `call.files.write(path, content)` and `remove(path)`
  (a string, `Uint8Array` or `ArrayBuffer`; at most `FILE_WRITES_MAX` files
  and `FILE_WRITE_MAX_BYTES`, each an effect; a path of at most
  `PATH_MAX_BYTES`; never a blob pointer's text), one commit once it
  commits. Last writer wins.
- In a job, `job.files.read / list / stat / write / remove` are steps;
  `write(path, content, {expect})` and `remove(path, {expect})` compare and
  swap on the blob `sha` (`null`: must not exist), failing with `conflict`.
- A move of `main` by the cell's own commit carries its writer's depth.

### Blobs

A file of 1 MiB or more is a git-lfs v1 pointer in git, its bytes a blob in
the fleet's store under the fragment and the hash. The CLI uploads a large
file's bytes before it commits the pointer and downloads them as it pulls,
where status answers `blobMinBytes`. The site and `GET file` serve a
pointer's bytes, with ranges. A blob no pointer at `main` or `live`, and no
record's `attachments` (`[{"sha256": …}]`), has named for
`FRAGMENT_BLOB_GRACE_S` is deleted, uploads never committed included.

A page reads a blob at `__blob/<sha256>` and an editor's page uploads one
there (`fragment.blob(file)`). It is served as the type its upload
declared when that is passive media (`blob::served_type`: JPEG, PNG, WebP,
GIF, MP4 and WebM video, MP3, WAV, WebM, Ogg and MP4 audio, PDF), else
`application/octet-stream`, so a blob never runs as a page or script.

### Cards (docs/cloudflare-v1.md, decision 31)

Every move of `live` wants a **preview card**: a 1280×800 JPEG shot of the
fragment's page, which the shell's Apps page shows (`fragment_core::card`,
cell/src/card.rs).

- **Off the deploy's path.** The fragment's alarm takes the shot with the
  `BROWSER` binding, one at a time, the newest live winning; a failed try
  is retried with backoff, at most `card::ATTEMPTS_MAX`, then given up
  (`card.failed`) and the card before stays.
- **As a visitor sees it.** The renderer holds no one's session: a
  `public` fragment as anyone sees it, a `link` one as its share link's
  holder; a `members` fragment, a chat and an agent fragment are not shot
  (`card.skipped`). The card goes only to members.
- **What the page reported** as the shot loaded it, `status.page` `{live,
  at, errors: [{kind: exception | console | network | security, text,
  source}], dropped}`: its first 10 errors, the share link's token replaced
  by `[share link]`; `page.errors` when it has any.
- **An editor's asks** (Control API): `POST card` wants a shot of the live
  now, replacing the card, an upload too (`joined` when one was wanted
  already); `PUT card` makes their own image the card until a shot is asked
  for again or another is uploaded (later deploys are still shot, for the
  report).
- **Metered** to the owner, each try's browser time (docs/ledger.md). An
  owner whose ledger refuses new things gets no shot (`card.skipped`).
- **Events:** `card.made` `{live, blob, attempt}`, `card.uploaded` `{blob,
  size}`, `card.skipped` `{live, why: not_an_app | members_only |
  owner_pays}`, `card.failed` `{failures}`, `page.errors`.

A page's Open Graph image stays `__preview.svg`: the card is its members'.

### Icons (issue #587)

A fragment's **icon** is one 256×256 PNG of one object on a transparent
background, which the shell's dock shows and its pages take as their
favicon (`fragment_core::icon`, cell/src/icon.rs). An agent fragment has
none (its picture is its `avatar.png`): a making or `PUT` on one is 400.

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/f/{name}/icon/make` | editors | `{hint?}` (at most `icon::HINT_MAX_CHARS`) → `{subject, candidates: [{png, score}], flaws}` |
| `PUT /api/f/{name}/icon` | editors | the image's bytes → `{icon, sha256, size}` |
| `GET /api/f/{name}/icon` | viewers | the PNG (`ETag` its SHA-256), or 404 |
| `DELETE /api/f/{name}/icon` | editors | → `{icon: null}` |

- **A making** names one object for the fragment's title, description and
  the hint, draws `icon::CANDIDATES` of it, cuts each out and checks it,
  within a minute. `candidates` are those kept, best first, each `png` the
  icon's bytes as base64, `score` how surely it shows one object of its
  subject; `flaws` counts the refused by flaw: `empty`, `cut_off`, `tile`,
  `pieces`, `ragged`, `writing`, `framed`, `no_object`, `off_subject`,
  `failed`. Nothing is kept: its asker keeps one with `PUT`. At most
  `icon::MAKINGS_PER_HOUR` an hour a fragment (429).
- **An upload** is a PNG, JPEG, WebP or GIF of at most `BODY_MAX_BYTES`
  (413). A 256×256 PNG within `PICTURE_MAX_BYTES` is kept as it is; any
  other is fitted inside 256×256, and 413 if still too big.
- **Metered** to the owner (docs/ledger.md), asked first as a job's AI step
  is: a guest's fragment makes none, one at its cap none for anyone but its
  owner and their agents (402). `PUT` and `DELETE` past the overdraft 402.
- **Kept** as one of its blobs, named in its meta. Its members' rows show it
  as `picture`, `/api/f/<name>/icon?v=<12 hex of its SHA-256>`, cacheable a
  day; a stale version is 404 `no-store`.
- **Its pages' favicon.** A `GET` or `HEAD` of `favicon.ico` on its origin,
  where live has no such file and the app answers 404 there, is answered
  the icon, `no-cache`, its SHA-256 the `ETag`.
- **Events:** `icon.made` `{subject, offered, flaws}`, `icon.set` `{sha256,
  fitted}`, `icon.removed`.

### Deliveries: web push

A page subscribes with `await fragment.push.register(who)` (from a click:
it registers `./__sw.js`, reads the VAPID key at `./__push-key`, and stores
the subscription at `./__push-sub` tagged `who`);
`fragment.push.unregister()` drops it (`./__push-unsub`). A chat framed by
the shell stores a subscription the shell made, at the same routes (The
shell). Anyone who may see the fragment may subscribe (at most
`PUSH_SUBS_MAX`); a `who` that is an npub is that identity's own (403 from
anyone else). `call.push(who, payload)` in a mutation (sent once it
commits) and `job.push(who, payload)` in a job (a step answering
`{queued}`) push `{title, body, tag, url, icon?}` (at most 3800 bytes) to
the subscriptions tagged `who` (at most `PUSH_WHO_MAX_CHARS`), or `*`; a
relative `url` is the fragment's own, and `icon` an `http(s)` picture.

Every delivery (a record to a subscription, a push, a computer's wake) is
written to the fragment's outbox with what caused it, then sent; one not
taken is tried again (`delivery.deferred`), and given up as
`delivery.failed`. Delivery is at least once: a record subscriber dedupes
by channel and `seq`. A push service's 404 or 410 drops the subscription
(`push.gone`).

### AI

A job's AI steps bill the fragment's owner, capped when the run's
principal is neither the owner nor an agent of theirs (Ledger). Text goes
through the model route by tier (Models), images to a Workers AI image
model, decisions to the Clef family (Decisions). No app holds a key.

Each paid step asks the gate before its call (docs/ledger.md, "The gate";
decision 62); nothing is held. The step keeps what its call bought, and its
charge, by `<fragment>@<incarnation>/run/<run>/step/<index>`, so a step
tried again after its call answered reuses it and never calls or pays
again, and a replayed run pays only for the rest. A step that fails for
good is charged nothing. A step the owner's ledger refuses fails with its
reason (402 `budget_used_up`). A run answers `costMicros`.

- `job.ai.text({model?, prompt | messages, max_tokens?, reasoning_effort?,
  tools?, tool_choice?, draft?})` → `{text, message, finish_reason, model,
  tier, usage, timing}`: `model` a tier, `cheap` (the default) or `medium`
  (`high` refused); `max_tokens` at most 16384; `reasoning_effort` `low`
  (the default, and what anything unknown becomes), `high` or `max`.
- `job.ai.decide({model?, input, questions, media?})` → `{model, answers,
  usage}` (Decisions). An answer that does not answer what was asked is
  kept as a rejection (`ai.decide-refused`), charged, and thrown; a retry
  reuses it. No fallback.
- `job.ai.image({prompt, path, steps?})` → `{path, size, sha256,
  mediaType}`: a JPEG written to `main` at `path` (ending `.jpg` or
  `.jpeg`), a blob from 1 MiB. `prompt` 1 to 2048 characters, `steps` 1 to
  8 (default 4); any other key refused. An answer that is no JPEG is
  refused and charged nothing (`ai.image-refused`).
- `job.ai.video({prompt, path, duration?, resolution?, aspect_ratio?})`
  → `{path, size, sha256, mediaType, lengthMs}`: an MP4 written to `main`
  at `path` (ending `.mp4`), a blob from 1 MiB, made by HappyHorse 1.0
  (`alibaba/hh1-t2v`, Cloudflare's model catalog, zero data retention)
  through the deployment's AI Gateway. `prompt` 1 to 2500 characters,
  `duration` 3 to 15 seconds (default 5), `resolution` `720p` (the
  default) or `1080p`, `aspect_ratio` `16:9` (the default), `9:16`,
  `1:1`, `4:3` or `3:4`; any other key refused. Charged its seconds at
  its resolution's price as soon as the model answers with its video
  (a step tried again reads that video, and is never made or charged
  again); an answer with no completed video is refused and charged
  nothing (`ai.video-refused`). Asked at most once an attempt: a try
  that never heard its answer fails, charged nothing, saying to replay
  (`ai.video-lost`).

A text model's 429 or 5xx is answered by the fallback model (Models), sent
the same request; when that fails too, the step is retried. Other refusals
fail the step with the model's message.

#### Text steps: tools, drafts, timing

A text step is one OpenAI-shaped chat completion, streamed and read to its
`[DONE]`, so a job can run a small tool loop: call the model, run its own
code for each tool call, call again (an example: cli/GUIDE.md).

- **Messages** are OpenAI's: `system`, `user`, `assistant` (with
  `tool_calls`, as a step's `message` answers them) and `tool` (`{role:
  "tool", tool_call_id, content}`).
- **Tools** are OpenAI's functions, `{type: "function", function: {name,
  description?, parameters?, strict?}}`: at most `models::TOOLS_MAX`, in
  `models::TOOLS_MAX_BYTES` of JSON, each named 1 to 64 letters, digits, `_` or
  `-`, once; `parameters` an object's JSON Schema; any other key refused.
  `tool_choice` is `none`, `auto`, `required` or `{type: "function",
  function: {name}}`; refused with no tools. The fallback gets the same.
- **Size.** A step's arguments are at most `models::AI_TEXT_MAX_BYTES` of JSON
  (`job.ai.text` throws past it).
- **Steps.** Each call is a step: a run takes at most `JOB_STEPS_MAX`,
  tools' own steps among them. Bound the loop in the job.
- **The answer.** `text` (`""` when it only called tools); `message`
  `{role: "assistant", content, tool_calls?}` (each `{id, type: "function",
  function: {name, arguments}}`, `arguments` the model's JSON text, which may
  not parse), never its reasoning; `finish_reason` (`stop`, `length`,
  `tool_calls`); `model` the model that answered; `usage` as reported.
- **Timing.** `{first_ms, ms, model, calls, tries}`: to the first data
  line, to `[DONE]`, the model that answered, calls this try made (2 when
  it fell back), and the step's Workflow try (`null` when unknown). A
  step tried again answers the kept one.
- **Drafts.** With `draft: {channel, turn}` the text so far streams into
  that channel's drafts from the run's principal (a channel the app
  declares, no `post` role needed, never a built-in one; `turn` as a
  draft's): `{text: "", thinking: 0}` as it begins, `thinking` (ms) at most
  once a second until words come, then the text so far at most 4 times a
  second (none past 64 KiB), and whole at its end; `text: null` if it
  fails. The job then publishes the record with the same `turn`, which
  replaces the draft on a page.
- **Prices.** Tools are tokens like any other (docs/ledger.md).

### Decisions

A decision answers typed questions about text, JSON, images, audio or
video with probabilities, never prose: a job asks it as
`job.ai.decide`, an agent at `POST /api/models/v1/decide` (Models), one
contract (`fragment_core::decide`), on Workers AI's Clef family. The
platform maps it to Clef's own wire, which apps never see.

**The request**, `{model?, input, questions, media?}`; any other key is
refused, and everything is checked before anything is sent:

- `model`: `clef-flash` (the default; `@cf/cloudflare/clef-flash`, text and
  images), `clef` (`@cf/cloudflare/clef`, text and images) or `clef-omni`
  (`@cf/cloudflare/clef-omni`, also audio and video).
- `input`: nonempty text, an object or an array.
- `questions`: 1 to 64 ids (1 to 100 of ASCII letters, digits, `_`, `.`,
  `-`), each one of:
  - `{type: "choice", instructions, options}`, `options` mapping 2 to 255
    ids (at most 100 bytes) to descriptions → `{type: "choice", choice,
    probabilities, confidence}`;
  - `{type: "predicate", instructions}` → `{type: "predicate",
    probability}`;
  - `{type: "score", instructions, levels}`, 2 to 10 levels lowest first →
    `{type: "score", score, levels, probabilities, confidence}`, `score` the
    probability-weighted zero-based level, `probabilities` keyed `"0"`,
    `"1"`, ….

  Instructions and descriptions are nonempty. Input and questions encode
  to at most 16 KiB on `clef-flash`, 48 KiB on the others.
- `media`: at most 4 items, at most 2 videos, each a blob of the asking
  fragment `{sha256, type}` (a job's only; extra `size` and `name` ignored)
  or inline `{data}`, a base64 `data:` URL (a job's at most 256 KiB in all).
  Types: `image/png`, `image/jpeg`, `image/webp`; `audio/wav` (`audio/x-wav`,
  `audio/wave`), `audio/mpeg` (`audio/mp3`); `video/mp4`, `video/webm`
  (`clef-omni` only). At most 4 MiB an image, 8 MiB an audio or video, 8
  MiB in all, decoded. No remote URL.

**The answer**, `{model, answers, usage: {input_tokens, output_tokens}}`,
`model` the catalog id. Each answer is checked against its question before
it leaves: probabilities in [0, 1] summing to 1 (within 0.001), a choice
of highest probability, a score matching its distribution (within 0.01).
It is charged after the answer at its input tokens (docs/ledger.md).

### Models (docs/cloudflare-v1.md, decisions 23 and 61)

One OpenAI-shaped chat completion on a tier's model, metered on its
payer's ledger (cell/src/models.rs, `fragment_core::models`). The route's
names: the tiers `cheap` (the default) and `medium`; `high`, refused (400,
saying why); `vision`, for a runtime's calls about an image; and
`fallback`, a runtime's own switch when its model stalls. `vision` and
`fallback` are metered as a tier's call and are no tier a job's step may
name. Where each runs is the deployment's config (`FRAGMENT_MODELS`;
decision 61): a Workers AI model through the deployment's AI Gateway, or a
model on an OpenAI-compatible API with the deployment's key and its slot's
own `body`, the call's other fields than a chat completion's dropped
(`models::CLIENT_KEYS`).

**Fallback.** A tier's call whose model fails before answering (not
reached, 429, 5xx; on an OpenAI-compatible API also 401, 402, 403, 404,
none of them the caller's) is made once more on its route's fallback, and
its answer is the call's, charged once (a 401 or 402 is logged,
`model.platform-key`). An answer that began is final; a 400 or other 4xx is
the call's. `vision` falls back only to a model whose slot says it reads
images; transcriptions never. A slot may name its own `"fallback"`, its
Deep (`"deep"`), `"reasoning"` and `"max_tokens"` (docs/computers.md,
"Fast and Deep"); a caller may say its tier in `x-fragment-tier` (`cheap`
or `medium`; any other 400).

**A model by its id.** A call may name a slot's model by its own id instead
(`Models::named`): the caller's tier's slot that runs it, else its Deep,
else the first slot that does (`cheap`, `medium`, `fallback`, `vision`),
as if it named that slot. An id no slot runs is 400, the message listing
those a call may name. One id is one model: a deployment whose two slots
name one id at two places is refused. A job's step names a tier, never an
id.

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/models/v1/models` | an agent | → OpenAI's list, `{object: "list", data: [{id, object: "model", created: 0, owned_by: "fragment"}]}`: the ids its calls may name |
| `POST /api/models/v1/chat/completions[?fragment=<name>]` | an agent (`for` its asker) | an OpenAI chat completion, `model` a name or an id → the model's answer in OpenAI's shape: JSON, or with `stream: true` server-sent events, usage once on a last chunk with no choices |
| `POST /api/models/v1/audio/transcriptions` | an agent | `multipart/form-data`: `file` (the audio, as recorded), `model` `whisper`, optional `language` (ISO-639-1), `prompt`, `response_format` (`json`, the default, or `text`) → `{"text": …}`, or the text |
| `POST /api/models/v1/decide[?fragment=<name>]` | an agent (`for` its asker) | a decision, media inline (Decisions) → `{model, answers, usage}`. A blob 400 (the route reads no fragment's store). An answer that does not answer what was asked is charged and 502 `upstream_failed`. A computer's guest asks it as `POST /v1/decide` on its intercept |
| `POST /api/models/v1/embeddings[?fragment=<name>]` | an agent (`for` its asker) | OpenAI's `{input, model?, dimensions?, encoding_format?}` → `{object: "list", data: [{object, index, embedding}], model, usage, space: {model, dimensions, version}}` (Embeddings). `POST /v1/embeddings` on a computer's intercept |

The model is sent the body bounded: its tier's model, images as they came,
`max_tokens` at most 16384 (or its slot's), `reasoning_effort` clamped,
usage asked for when it streams, none of the client's headers. Its payer is
the agent's owner (decision 36). `fragment`, one the agent is a member of
(403 otherwise), is where the turn is: when its owner pays, the call counts
in its month and under its cap; when another person owns it, that owner's
ledger must still be open (decision 26). Both are asked through the gate.
A refusal of the payer's ledger is 402 `budget_used_up`, 403 for a guest.

Each call is charged after its answer at its last cumulative usage, one
meter row under its own `aig:<hex>` reference, a streamed call at its
`[DONE]` (docs/ledger.md). A call the model refuses, or never reached, is
charged nothing and passed through; one whose usage never came is charged
nothing, said aloud (`ledger.unknown-usage`). Nothing of a request or its
answer is kept. Each charge logs `{"event": "model.settled", ref, logId,
tier, model, usage, first_ms, ms, calls}`.

**Embeddings** (`fragment_core::embed`) run on the deployment's
`models.embeddings` slot, no chat slot (no tier names it, nothing falls
back to it); unnamed, `embed::MODEL_DEFAULT` on Workers AI. Every answer
names its `space`, and every vector has the slot's `dimensions` and unit
length;
a change of the slot's model, dimensions or version is a new space, never
compared with the old. A slot may name a `document_prefix` and a
`query_prefix` (each at most `embed::PREFIX_TOKENS_MAX` tokens): the task
instructions a fragment's search puts before each chunk and each question
it embeds (nomic-embed-text's `search_document: ` and `search_query: `),
which its index's `space` names, so a change of either is a new space
there too. This route embeds an agent's input as given. A deployment whose
tiers are private embeds only on a slot marked `private` (else 403). Refused 400 before anything is sent:
`input` not a string or 1 to `embed::INPUTS_MAX` strings, an empty one, or
one that may pass the slot's window (`embed::tokens_max`; nothing is cut);
`model` other than `embeddings` or the slot's; `dimensions` other than the
slot's; `encoding_format` other than `float` or `base64`. A body past
`embed::BODY_MAX_BYTES` is 413. An answer of another length or count is
502, and charged. It is paid as a chat call, at its input tokens (the
model's count, else estimated).

**Transcription** (decision 9): `whisper` is Workers AI's Whisper, sent the
audio as base64 with its voice-activity filter on; `language` and `prompt`
pass on, OpenAI's other fields do not. A form that is none, another model
or format is 400; audio past `transcribe::BODY_MAX_BYTES` 413. Its payer is
the agent's owner, under no fragment's cap, charged by the audio's length
(`fragment_core::transcribe`). A first try answering 429, 408 or 5xx, or
nothing within 15 s, is made once more; a 429 passed on carries
`retry-after: 5`. Every answer carries `x-fragment-answer-shape` (the
model's own answer's top-level keys and small `usage`, never its words). An
answer in another shape is 502, charged nothing.

### Ledger (docs/ledger.md)

Every person has a usage ledger, one `Ledger` object named by their
identity (`fragment_core::ledger`): plans, credit, caps and meters in
integer micro-dollars over UTC months. Plans (decision 25): `guest`,
`seat`, `seat_always_on`; a new person's is `FRAGMENT_DEFAULT_PLAN`. What
the wire shows of its rules:

- **At zero** (decision 27) agents stop: no turns, no AI steps, the
  refusal saying why. Fragments keep serving and taking writes.
- **Past the overdraft** their fragments are read-only until a top-up:
  mutations, jobs, posts, file writes, deploys, storage tokens, blob
  uploads, inbox deliveries and replays are 402 `budget_used_up`; reads
  serve; triggers record `blocked` runs; no new fragment is made.
- **Guests** make no fragments (403); they edit those shared with them,
  whose owners pay.
- **Caps** (decision 26): past a fragment's monthly cap, AI steps and agent
  turns there stop for everyone but its owner and their agents acting for
  them. Caps never stop writes.
- **An org's ledger** (decisions 66 to 68), `org:<id>`, holds the org's
  own credit, operators' grants (orgs pay out of band). It has no plan,
  seat, included credit or overdraft (its fragments are read-only at
  zero), pays for no agent or computer, and has a monthly budget an
  operator sets (zero until set), past which its fragments' AI steps stop.
  Every AI step in its fragments meets the fragment's cap, whoever starts
  it.
- A fragment asks its owner's standing through the gate, so a change
  reaches it within a minute; a ledger that does not answer refuses
  nothing.

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/ledger` | a person (an agent: its owner's) | → `LedgerStatus {plan, seat, month, balanceMicros, includedMicros, includedGrantedMicros, purchasedMicros, reservedMicros, availableMicros, overdraftMicros, standing: {standing: ok \| agents_stopped \| read_only, why?: guest \| seat_canceled \| no_credit \| overdrawn}, priceBook, fragments: [{fragment, spentMicros, capMicros}]}`: `balanceMicros` negative while they owe; `reservedMicros` always 0 (decision 62); `availableMicros` what may be charged before read-only; `priceBook` the version the answering release charges with |
| `PUT /api/f/{name}/cap` | the fragment's owner (never an agent) | `{id, micros \| null}` → `{fragment, capMicros, default}`; `null` is the default |
| `POST /api/ledger/{person}/grant` | operators | `GrantCredit {id, micros, by, why}` → `{}`: purchased credit; `by` the signing operator |
| `POST /api/ledger/{person}/plan` | the same | `SetPlan {id, plan}` → `{}`: an override; a seat holder's next push wins (debt ledger) |
| `POST /api/ledger/{person}/overdraft` | the same | `SetOverdraft {id, micros}` → `{}` |
| `POST /api/ledger/org:{org}/budget` | the same | `OrgBudget {id, micros}` → `{}`: the org's monthly budget, at most `ledger::org::BUDGET_MAX`; a person's ledger 403 |
| `GET /api/orgs/{org}/ledger` | the org's billing admins, in person | → `OrgLedgerStatus {org, month, balanceMicros, budgetMicros, spentMicros, standing, priceBook, fragments}`; anyone else 403 |
| `GET /api/admin/orgs/{org}/ledger` | operators | → `OrgLedgerStatus` |

`{person}` is an email, an npub, or `me`; for a grant or a budget,
`org:<org id>` too (an org that is there, else 404; its ledger refuses a
plan or an overdraft, 403). Commands are idempotent by `id` (per command
kind): the same id again changes nothing, with another body 409
`conflicting_body`; a body breaking a rule is 400 and remembered by
nothing. Any other command is 404: a seat is the registry's alone
(`/api/admin/seats`). Each is logged (Operators: the admin); an org's is
in its governance log too (Org content), written, its body checked,
before its ledger takes it, so one cut short is finished by sending it
again.

### Seats and orgs (docs/billing.md)

A seat is what lets a person create (decisions 49 and 51):
`seat_always_on`, the one seat sold and every buying body's default
(`seat` there is 400), or `seat`, an operator's comp only. Every seat is
in an org, which pays; a person is in at most one org and holds at most one
seat. An org's members (`OrgMember`) are its admins, its seats' holders,
or both: each a person who said yes, or an **offer** to an email (`person`
null). Nobody joins an org without their own yes: whoever signs in with an
offer's email, verified, accepts or declines it. A seat is **comped** (an
operator's; always good) or **paid** (on the org's Stripe subscription,
good while it is `trialing`, `active` or `past_due`). **Who pays** is the
person whose Checkout made the subscription (`OrgView.payer`,
`MySeat.pays`): while it lives no other admin removes them (403), only
they open the portal, and their leaving, stepping down or moving is 409
(cancel it first). The registry is the one writer of a seat holder's plan
and seat, pushed to their ledger and computer.

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/seat` | a person | → `SeatStatus {seat: SeatView? {id, org: {id, name}, kind, comped, good, sleeps, admin}, org?, admin, pays, offered: [OfferedSeat {id, org, kind?, admin, comped, from?, email}], guest, offer?: {kind, seatMicros, includedMicros, packMicros}}`: `guest` whether they are one (`fragment_core::org::guest`); `offer`, what is sold (`fragment_core::stripe::offer`), null where nothing is (no Stripe) |
| `PUT /api/seat` | a seat's holder | `{sleeps}` → `MySeat`: whether their always-on computer may sleep; no seat 400 |
| `POST /api/seat/offers/{id}/accept` | whom the offer names (anyone else 404) | `{}` or none → `MySeat`, the offer theirs. From another org, a move: 409 for that org's last admin while others are in it, or its payer while its subscription lives; alone there, it ends. An admin's offer of an org that pays for nothing is 400. Again: the same answer |
| `POST /api/seat/offers/{id}/decline` | the same | `{}` or none → `MySeat`; answered already 404 |
| `POST /api/seat/leave` | a holder who is no admin | `{}` or none → `MySeat`: their seat canceled. The payer 409; an admin 400 (step down first); in no org 400 |
| `GET /api/org` | an org's admin | → `OrgView {id, name, createdAt, members: [OrgMember {id, person?, email, admin, seat?, comped, addedAt}], payer?}`; anyone else 403 |
| `POST /api/org/seats` | an org's admin, while it pays (else 400) | `{email, kind?}` → `{seat: OrgMember, created, mailed}`: a paid seat offered to the email, billed from the offer. A member holding a seat 409; a person being wiped 404; an email `org::OFFERS_PER_EMAIL_MAX` orgs offer to 400; past 50 offers mailed by the org that UTC day 429. Again: `created: false` |
| `DELETE /api/org/seats/{id}` | an org's admin | → `OrgMember` as it was: the seat or its offer gone. The payer's own seat 403 to others; the last paid seat 400 (cancel in the portal) |
| `POST /api/org/admins` | the same, while it pays | `{email}` → `{admin, created, mailed}`: an admin's place offered (theirs only by their yes); at most 10, offers counted |
| `DELETE /api/org/admins/{id}` | the same | → `OrgMember` as it was. The last admin who said yes 409; the payer 403 to others, and 409 to themselves while it lives |
| `POST /api/admin/seats` | operators | `CompSeat {email, kind, org?}` → `Comped {seat, org, created, mailed}`: in `org` (none: 404), else the person's org, else a new org of one. A person's comp is theirs at once; an email no one signs in with is offered it. Holding a seat already 409 (the same comp answers again, `created: false`); in another org than named 409 |
| `PATCH /api/admin/seats/{id}` | the same | `{kind}` → `OrgMember`; a paid seat 400 |
| `DELETE /api/admin/seats/{id}` | the same | → `OrgMember` as it was: the comp ends; none 404 |
| `GET /api/admin/orgs/{id}` | the same | → `OrgView` |
| `POST /api/admin/trials` | the same | `NewTrialCode {name, kind?, days (1-30), capacity (1-10,000), expiresAt?, code?}` → `TrialCode {id, code, name, kind, days, capacity, expiresAt, active, revision, createdAt, createdBy, subscribed, open, uses}`: `kind` only `seat_always_on`; no `code`, one is made (16 of A-Z less I and O, and 2-9, in fours); taken 409 |
| `GET /api/admin/trials` | the same | → `{codes: [TrialCode]}`, newest first |
| `GET /api/admin/trials/{id}` | the same | → `TrialCode` with `uses: [{person, email, at, status}]` |
| `PATCH /api/admin/trials/{id}` | the same | `{revision, capacity?, active?, code?, expiresAt?}` → `TrialCode`, when `revision` is still its (409); `capacity` never below those taken (400) |
| `POST /api/admin/orgs/{org}/payer` | the same | `NamePayer {id, person}` → `PayerNamed {org, person, email?, seq}`: whom the org's credit is invoiced to (an email they sign in with, or their npub; else 404), logged in the org's governance log and the operators', once by `id` (another body 409). It gives them no power |

### Org content (decisions 69 to 71)

An org's content is apart from its billing (`fragment_core::governance`):
a seat or a billing admin's place gives none of it, and a seatless person
may hold it. Its people (`ContentPerson {person, email?, admin, since}`)
are in by their own yes; its **content admins** govern it. Each call is a
person's own (an agent is 403), and every change is in the org's
**governance log** in the same transaction, naming people by npub, never
by email. A client's `id` makes a call idempotent (another body 409).

- **Appointed, never automatically.** An admin's place is an offer to an
  email, answered once by whoever signs in with it, while the grounds it
  was made on hold: an admin's offer, while they are one; who pays for it,
  for its first, while its content has had no one; an operator's, while
  it has no admin. An offer expires after `OFFER_TTL_MS`; one whose grounds
  no longer hold lapses, named in the log. At most `ADMINS_MAX` admins,
  offers waiting counted.
- **The last admin stays**: stepping down never leaves the org without
  one (409).
- **It outlives its billing**: an org with anyone in its content, or that
  an operator credited or named a payer for, is never dropped when its
  seats go.
- An offer is mailed to no one yet: its person reads it at `GET
  /api/orgs/offers`.

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/orgs/{org}/content-admin-offers` | its content admins; who pays for it, for its first | `NewContentAdmin {id, email}` → `ContentOffer {id, org, email, grounds, from?, at, expiresAt, answer?}`; an email waiting already answers that offer. Anyone else 403; past `ADMINS_MAX` 400 |
| `GET /api/orgs/offers` | a person | → `ContentOffers {offers}`: those waiting on their emails |
| `POST /api/orgs/{org}/content-admin-offers/{offer}/accept` | whoever signs in with its email (anyone else 404) | `{}` or none → `ContentOffer`: they are its admin. Again, the same; declined, or its grounds gone, 409; withdrawn, expired or lapsed 404 |
| `POST /api/orgs/{org}/content-admin-offers/{offer}/decline` | the same | `{}` or none → `ContentOffer`; accepted 409 |
| `DELETE /api/orgs/{org}/content-admin-offers/{offer}` | its content admins | → `ContentOffer`, `answer: withdrawn`; answered 409, expired 404 |
| `DELETE /api/orgs/{org}/content-admins/{person}` | the same | → `ContentPerson`: an admin no more, still in its content, their offers withdrawn. The last admin 409 |
| `GET /api/orgs/{org}/content?before=` | the same | → `OrgContent {org, people, offers, log: [GovernanceEntry {seq, at, actor, action, subject?, request, detail}], next?}`, the log newest first, `LOG_PAGE` a page |
| `POST /api/admin/orgs/{org}/content-admin-offers` | operators, while it has no content admin (else 403) | `NewContentAdmin` → `ContentOffer`, logged there and in the operators' log |
| `GET /api/admin/orgs/{org}/content?before=` | the same | → `OrgContent` |

### Billing (docs/billing.md, "Stripe" and "Trials")

A deployment that sells seats (its config's `stripe`; else these are 400)
sells one always-on seat a month through Stripe Checkout (its price's
lookup key `fragment_seat_always_on_month`), and a credit pack
(`fragment_credit_pack`, decision 55). Nothing is made before it is paid.
An org has one Stripe customer and one subscription; everything fragment
makes carries `fragment_deployment` (the platform's origin) in its
metadata, and nothing without this deployment's is touched. The registry
writes an org's subscription only as fetched from Stripe (the Checkout's
return, the webhook, a daily refetch); `trialing`, `active` and
`past_due` are good, anything else lapses the org's paid seats. No paid
seat left cancels the subscription at once. A Checkout paid whose buyer
can no longer take its seat seats no one and is canceled.

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/billing/checkout` | a person (holding a good seat, or in an org whose admins give seats: 409) | `{kind?, trialCode?}` → `{url, session}`: Checkout of one always-on seat (an org buying again: its paid seats), for 31 minutes. With a trial code (any case, spaces and dashes aside), Stripe's trial of its days, no card asked, no promotion code. A code unknown 404; ended 400; full, or for someone who had a trial or whose org has paid, 409 |
| `POST /api/billing/sessions/{id}` | its buyer (anyone else 404) | → `MySeat`: the Checkout's return (`/welcome?checkout={id}`, `/settings?pack={id}`; `=canceled` when abandoned): a completed one applied at once; again, nothing changes |
| `POST /api/billing/packs` | a seat's holder; an org's admin for a member holding a seat (`member`; else 403; no seat 400) | `{member?}` → `{url, session}`: a credit pack's Checkout; paid, its credit is granted once by its session (`GrantCredit {id: pack:<session>, by: stripe}`). An org buys none: an operator credits it (Ledger) |
| `POST /api/billing/portal` | who pays for the asker's org (anyone else 403) | → `{url}`: Stripe's portal on the deployment's own configuration |
| `POST /api/stripe/webhook` | Stripe (`Stripe-Signature`, this endpoint's secret, within 300 s; else 401) | an event → `{received, applied}`: `checkout.session.completed` and `customer.subscription.created`, `.updated`, `.deleted` are fetched again and written; anything else, or another deployment's, answered and dropped. A failure is 5xx, which Stripe sends again |

### Jobs and triggers

A job is a method called `(input, job)` that runs as a Cloudflare
Workflow, outside any request. Every kind of step is `Step` in
`crates/core/src/steps.rs`. Besides the files steps, `job.push` and the AI
steps:

- `job.call(op, input)`: an operation of this fragment as the run's
  principal, its id `job:<run>:<step>`. Calling a job starts it (`{run,
  status}`), one hop deeper.
- `job.fetch(url, {method, headers, body})` → `{status, ok, headers,
  text(), json()}`, the app's one way out. `{{NAME}}` in a header value,
  or in the URL's path or query, is the secret `NAME`, filled at the
  egress toward its hosts only (Control API); a fetch elsewhere fails the
  step, naming both. A URL's is percent-encoded as it goes in, a value
  that would change where the request goes (host, port, path segments,
  query parameters) fails the step unsent, and the answer and a
  failure's message name every value filled, a header's or the URL's,
  as its placeholder again, however the vendor spells it back
  (percent-encoded, JSON- or HTML-escaped, in base64: docs/secrets.md);
  `{{name}}` naming no secret goes as written
  (`fragment_core::secrets`). A connector bound as a door that is shut
  fetches nothing (An agent's doors). http(s) only;
  loopback, private and `.internal` addresses refused; redirects answered,
  not followed; a body of at most `FETCH_BODY_MAX_BYTES`, a response of
  `FETCH_RESPONSE_MAX_BYTES`, `FETCH_TIMEOUT_MS`. Each carries
  `x-fragment-hops`, and one not GET or HEAD an `Idempotency-Key:
  <fragment>-<life>-r<run>-s<step>` (the same on every try) unless the job
  set one.
- `job.publish(channel, body, kind)`: a record.
- `job.sleep(ms | "N seconds|minutes|hours|days")`, up to 30 days.
- `job.members()`, `job.people(ids)` (at most 64), `job.presence()`: as the
  page reads `__members`, `__people` and its presence.

`job.principal`, `job.role`, `job.run`, `job.attempt`, `job.via` (`call`,
`job`, `cron`, `channel`, `files`, `installed`) and `job.fragment` say who and which;
`job.door` is a connector's `{relayed, to}` while it is an open door (An
agent's doors), else null.

The method re-runs from the top at every step with the answers so far, so
it must reach its steps in the same order and change nothing but through
steps (a step taken as another kind throws). Each step's answer is kept
before the Workflow hears it, so a step whose answer was lost is answered
from what was kept; one cut short before then runs again: **at least
once**. Calls, publishes, file writes, pushes and agent turns are keyed by
run and step, so a repeat applies nothing twice; a fetch's repeat carries
the same `Idempotency-Key`. A step failing for a reason that may pass is
retried 4 times (`FRAGMENT_JOB_RETRY_DELAY_S`, doubling); one failing for
good, out of retries, or whose arguments do not fit throws a `StepError`
the job may catch. A job that throws is **held** until replayed. A run uses
the code installed at its first step: a deploy that changes it restarts the
run at its next step as its next attempt (`run.restarted`), held at the
8th. At most 256 steps (`limits::JOB_STEPS_MAX`) and
`JOB_RESULTS_MAX_BYTES` of results a run; a result of at most
`RESULT_MAX_BYTES`.

Every job call and trigger is a **run**: `queued`, `running`, `succeeded`,
`held` or `blocked`. Triggered runs act as the fragment (its npub) with an
editor's role, at a `depth`: a record appended at depth d triggers at
d + 1 (a delivery's `x-fragment-hops`), and past `HOP_DEPTH_MAX` a run is
`blocked` (`cycle.detected`). An operation's triggers pause themselves
(`op.auto-paused`) after `AUTO_PAUSE_HELD` held runs in
`AUTO_PAUSE_WINDOW_MS` or `TRIGGERED_RUNS_PER_HOUR` triggered runs in an
hour; paused, a trigger records a `blocked` run. Past the owner's
overdraft every trigger's run is `blocked`. A deploy dropping an operation
drops its pause. A cron tick whose last run still goes is skipped. The
inbox's pending records are its unfinished runs (`inbox.rejected` past
`INBOX_PENDING_MAX`). Finished runs are kept `RUN_RETENTION_MS` (at most
`RUNS_KEPT`).

### Scheduled records

A fragment's editors schedule records at run time, with no deploy: a
scheduled record `{cron, channel, record}` appends `record` to `channel`,
as the fragment, at each minute `cron` names (UTC). Whatever follows the
channel follows it: triggers, pages, a computer's guest. An agent's
routine is one, on its own agent fragment's `tasks` (decision 38).

- Kept in the fragment (`record_schedules`), untouched by installs. A fire
  is keyed by its schedule and minute (`fragment_core::schedule::fire_key`),
  so it appends once; one reached late is one fire, missed minutes
  skipped. Past the owner's overdraft none fires (`schedules.blocked`); a
  channel no longer taking its records skips it (`schedule.unfired`).
- Viewers list them; editors add, pause, resume and remove them (an agent
  on its own fragment is its editor: no special case).
- **Bounds** (`fragment_core::schedule`), each a typed refusal: `cron` five
  fields read as triggers' are (weekdays by name too), at most every
  `RECORD_SCHEDULE_EVERY_MIN_MINUTES` (each fire wakes and costs its owner;
  more often belongs in a cron trigger), else `invalid_request`; `record`
  any JSON of at most `RECORD_SCHEDULE_BODY_MAX_BYTES` as canonical JSON,
  else `too_large`; `channel` declared, taking posts, and not only from one
  kind of member (`from`), else `invalid_request`, and the caller holding
  its post role, else 403; at most `RECORD_SCHEDULES_MAX` a fragment.
- The same schedule again answers the one there (`replayed: true`).
  Paused, it fires nothing and has no `nextAt`; resumed, it fires from its
  next minute. Events: `schedule.added`, `.paused`, `.resumed`,
  `.removed`.

A `Schedule` is `{id, cron, said, channel, record, createdBy, createdAt,
paused, nextAt?, lastAt?}`: `said` the schedule in words, `nextAt` and
`lastAt` in ms. A page reads them at `__schedules`. `fragment schedule
add|list|pause|resume|rm` (with `--routine` for an agent's) calls these.

## Serving

A fragment is served at `<name>.<suffix>/<path>`; `/f/<name>/<path>` on
the platform's host redirects there (`GET` or `HEAD`), and serves `__live`
and `__watch` in place for the CLI. Every path on a fragment's host is the
fragment's, `/api/…` included.

| path | |
| --- | --- |
| `/`, `/<page>` | `site/` in the live commit (`index.html` for a directory); Open Graph tags from `meta` |
| `__file?path=` | a content file from live, else main; an agent fragment's or a session archive's from main, else live (its computer, or its imports, write main and never deploy) |
| `__preview.svg` | the placeholder preview image |
| `__blob/{sha256}` | `GET`, `HEAD`: one of this fragment's blobs, to viewers and up (not `public` alone), as its declared type (Blobs; ranges 206), `private, max-age=31536000, immutable`, `nosniff`, `ETag` the hash; another fragment's hash 404. `PUT` (editors, from the fragment's own page) uploads as `PUT /api/f/{name}/blobs/{sha256}` → `{ok, sha, size, stored}`; another fragment's page uploads as no one (401) |
| `__members` | viewers and up (the share link too): as `GET /api/f/{name}/members` |
| `__schedules` | viewers and up (the share link too): as `GET /api/f/{name}/schedules` |
| `GET __channel/{channel}?after=\|before=&limit=` | the channel's readers: as `GET /api/f/{name}/channels/{channel}`, with this origin's session or share link |
| `POST __op/{op}` | a browser's call: `application/json` `{id, input}`; a signed-in browser calls as its person, an unsigned one gets an anonymous principal cookie; `public` callers get `PUBLIC_CALLS_PER_MIN` calls a minute each, `PUBLIC_CALLS_PER_MIN_FRAGMENT` a fragment |
| `POST __op/channels/{channel}` | a browser's post (`fragment.post`): `{id, input}`, the record's body as `input` → `{result: record, replayed}`; it spends a public call as a call does |
| `PUT __secrets/{KEY}` | an editor's session: `PUT /api/f/{name}/secrets/{KEY}`'s body and answer, `application/json` only (so no cross-site form sets one), from the fragment's own page; written, never read back (`secret.set` logs it) |
| `GET __door`, `POST __door` | its owner's session: `/api/f/{name}/door`'s, from a connector's own page (a `POST` as `application/json`) |
| `__signin`, `__signout` | this origin's session (Sign-in) |
| `__fragment.js` | the browser library (below) |
| `__fragment.css` | the platform's stylesheet (below) |
| `__markdown.js` | a module: `markdown(text, options)` → a DocumentFragment, `inline(parent, text, options)`; markdown to DOM only (HTML in the text is text; nesting stops at 32). Options: `resolve(target, kind)` → `{href, path?}` or null, `image(src, kind)` → `{src, href?}` or null, `code(text, lang)`, `breaks`; links stay http(s), mailto or relative, pictures http(s) or relative |
| `__people?id=…&id=…` | anyone who may see the fragment: `{profiles: {<id>: {kind, email?, picture, name?, fragment?, title?}}}` for at most 64 identities: `email` only between members (decision 48); `picture` an absolute platform URL; an agent's `fragment`, its label as `name`, its `title` and `picture` (An agent's name and picture); an unknown id left out |
| `__files` | the files viewer (`__files.js`, `__files.css`); asked for `application/json`, `{type: "files", count, files: [{path, size}]}`. Framed, it asks its parent to open a file as a window (`{fragment: "open", url, title}`) |
| `__live` | WebSocket, anyone who may see the fragment (below) |
| `__watch` | WebSocket, viewers and up (the share link, or a signed upgrade): `{type: "hello", ref, sha}`, then `{type: "changed", ref: "main", sha, paths}` per external move of main |
| anything else | the app's `fetch`, when it has one |

A site file carries an `ETag` naming its bytes (a page with Open Graph
tags: a weak tag naming the live commit too); the platform's own files
(`__fragment.js`, `__fragment.css`, `__sw.js`, `__files.*`,
`__markdown.js`) a hash of theirs. A matching `If-None-Match` is 304.

Every answer on a fragment's origin carries `X-Content-Type-Options:
nosniff` and `Referrer-Policy: same-origin`, over any the app sends. SVG
and XML answers (`image/svg+xml`, `application/xml`, `text/xml`,
`text/xsl`, any `+xml`), and `__file`'s HTML, also get
`Content-Security-Policy: sandbox`. The platform's host sends `nosniff`
on every answer, and `Referrer-Policy: same-origin` unless it names its
own. A computer's origin passes its guest's answers as they are.

**Sockets.** An upgrade on a fragment's host whose `Origin` is not the
fragment's own (`null` included) is 403; one naming no `Origin` is no
browser's, so its cookies count for nothing (it is its signer's, its
`?view=`'s, or anonymous). An unsigned visitor without a cookie gets the
anonymous principal cookie on the upgrade.

The `__live` protocol is JSON frames tagged by `type`, `LiveIn` (client →
server) and `LiveOut` (server → client) in `crates/proto/src/live.rs`:

- client → server: `{type: "subscribe", channel, after}` or `{type:
  "subscribe", channel, last}` (the last `last`, at most `CHANNEL_PAGE`;
  exactly one of the two), `{type: "unsubscribe", channel}`, `{type:
  "presence", data}` (at most `PRESENCE_MAX_BYTES`; `null` clears;
  `PRESENCE_PER_S` a second after a burst of `PRESENCE_BURST`, faster
  dropped with an error), `{type: "query", id, op, input}` (`id`
  `^[A-Za-z0-9._:-]{1,128}$`), `{type: "ping"}`.
- server → client: `{type: "hello", id, principal, role, platform,
  presence: [{id, principal, data}]}` (`platform` the platform's origin:
  the one page whose messages a framed page may take), `{type: "record",
  channel, seq, at, principal, kind, body}`, `{type: "subscribed", channel,
  next, more}`, `{type: "draft", channel, principal, turn, text, at,
  thinking?}` (never stored; `text: null` once stopped), `{type:
  "presence", id, principal, data}`, `{type: "changed", op}` (after every
  applied mutation), `{type: "result", id, result}` or `{type: "result", id,
  error, message, status}`, `{type: "pong"}`, `{type: "error", message}`
  (the socket stays open).

A subscribe answers one page of backlog (at most `CHANNEL_PAGE` records and
`CHANNEL_PAGE_MAX_BYTES`); with `more: true`, subscribe again from `next`.
The page that reaches the end makes it live, so records arrive in order.

A socket's role is fixed when it connects. Removing a member, rotating the
share link, or a fragment leaving `public` closes the sockets it admitted;
a role change closes theirs (and an agent's owner's) with 4001, after which
the library reconnects. A signed-in socket is checked again with the
registry at its next frame after `LIVE_IDENTITY_MS`: a sign-out, an ended
session or a revoked key closes it with 4001, as does a fragment waking
from hibernation or a registry that cannot answer. A page's unsigned
socket that the fragment refuses (401, 403) is opened and closed at once
with 4003, for good, rather than refused by status (a browser would dial
again, and Firefox holds back every socket to `/__live` after a refused
one). 4004: the fragment was deleted. A socket naming no page is refused
by its status. A fragment holds at most `LIVE_SOCKETS_MAX` sockets, a
principal `LIVE_SOCKETS_PER_PRINCIPAL` (429). Opening a socket at the
public role spends a public call.

A query over the socket runs as `__op` would, outside the public call
budget: a principal may run `LIVE_QUERIES_MAX` queries over its sockets
between two changes to the fragment, one at a time per id (429
`rate_limited` past it). A mutation or a job there is refused.

**The browser library** (`import * as fragment from "./__fragment.js"`):

- `call(op, input, {id?})` (retries keep the id); `post(channel, body,
  {id?})` (the record back; the same id, the same record);
  `observePosts(fn)` (this page's post attempts, `{channel, id, body,
  status: "posting" | "posted" | "failed", record?, error?}`).
- `live(op, input, onResult, onError?)`: a query re-run after every
  change, over the socket when open, else HTTP.
- `subscribe(channel, onRecord, {after?, last?, onDraft?, onReady?})`:
  pages the backlog, then follows live, resuming after a reconnect;
  `onDraft` hears drafts, `onReady({next})` the backlog caught up.
- `read(channel, {after?, before?, limit?})`: one page through `__channel`.
- `presence.set(data)` (changes within 150 ms sent as one), `presence.on(fn)`.
- `blob(file, {name?, type?})` → `{sha256, size, type, name}`, uploaded as
  an editor.
- `me()` → the hello's `{id, principal, role, platform}`; `closed(fn)`,
  called with `{code, reason}` after 4003 or 4004, which end the socket.
  Otherwise it reconnects by itself with jittered backoff.

**The stylesheet** (`<link rel="stylesheet" href="./__fragment.css">`,
`cell/fragment.css`) is for a page that links it; the platform adds it to
none. Its variables follow `prefers-color-scheme` through `light-dark()`
(a page may set `color-scheme` on `:root`, or `data-theme="light"` or
`"dark"` on `<html>`): `--bg`, `--fg`, `--muted`, `--faint`, `--line`,
`--line-strong`, `--surface`, `--soft`, `--accent`, `--on-accent`,
`--success`, `--danger`, `--font`, `--font-display`, `--font-mono`,
`--text-sm`, `--text-lg`, `--text-xl`, `--radius`. Their names hold across
releases; their values and the base styles may change. Base styles use
`:where()`, so a page's rules win.

### Hosts

The router takes a host in this order: the platform's own host (even under
the suffix); a fragment's host; the suffix's own name, which answers `308`
to the same path and query on the platform when the platform is elsewhere
(`no-store`); any other name under the suffix, 404; anything else, the
platform.

### Health

On the platform's host, for anyone, naming the release in
`x-fragment-deploy`:

- `GET /healthz` answers `ok`.
- `GET /healthz/objects` asks each platform object class once, at once
  (`routed::objects`): the Registry, and an object of each other class
  named `__probe`. Each answers right after its release check, before
  anything of its own: `{"release": <the Worker's>, "objects": {"Registry"
  | "Principal" | "Ledger" | "Fragment" | "Computer": {"release": <the
  Worker's>} | {"refusedBy": <its release>} | {"error": <why>}}}`, 200 when
  every one admitted the Worker's release with its schema ready, else 503.
  `cargo xtask deploy` asks it until 200. An object answers a probe only
  with its header (`x-fragment-probe`), which the router never passes
  from a client.

### The release's files

The files the platform serves as they are in its release (a blessed
template's site, data and code, a catalog template's files, the shell's
files, the browser files a fragment's origin serves (`__fragment.js`,
`__fragment.css`, `__sw.js`, `__files.*`, `__markdown.js`), the agent
docs) are the Worker's Static Assets, each named by its
SHA-256. The cell carries only their index (crates/templates) and reads a
file by its hash when it answers with it (cell/src/assets.rs). The cell
runs first on every request (`run_worker_first`), so a file is served only
by the routes above.

## Agent docs

The platform's host answers `GET /llms.txt` with `cli/SKILL.md` (what
`fragment skill` prints) and `GET /llms-full.txt` with `cli/GUIDE.md`
(`fragment guide`), from the release's files, so they never drift from the
CLI. Each names no host: its `{{platform}}` and `{{fragment}}` blanks are
filled by whoever shows it (`fragment_templates::agent_doc`), the cell with
its own platform and fragments' origin. They answer anyone, `text/plain;
charset=utf-8`, `no-cache`, an `ETag` of the file and what filled it. On a
fragment's host the same paths are the fragment's.

### Operations as MCP tools (`fragment mcp`)

`fragment mcp <fragment> [--write]` serves a fragment's described
operations as MCP tools over stdio (cli/src/mcp.rs, which states the
mapping; cli/GUIDE.md, "Your operations as tools"). The platform serves
nothing new for it: it reads `GET status` and calls `POST ops/{op}`,
signed as every CLI request is.

## Connections (decisions 22 and 37)

Every provider of the deployment's catalog (`FRAGMENT_PROVIDERS`,
`fragment_core::catalog`): a person's accounts (`connection`, held and
refreshed by WorkOS Pipes), the operator's keys (`operator`), and the
person's own keys (`own`). Their agents use them through the computer's
swap (docs/computers.md, Connections and operator keys). A connection's
authorization is browser-bound (docs/pipes-browser-binding.md).

| method & path | who | body → answer |
| --- | --- | --- |
| `GET /api/connections` | a person | → `Providers {providers: [{provider, kind, state, hosts, env, price?, signIn?, models?, account?, granted?, missing?, note?, accounts?, accountsMax?}]}`, in catalog order. `state`: a connection's `connected`, `needs_reauthorization` or `not_connected`; an operator key's `offered`; an own key's `set` or `not_set`. `price` an operator key's `{micros, per}`; `signIn` an own key's `{manage}`; `models` an own key's provider's `[{id, name}]`; `account` a connection's display email (shown, never trusted); `granted` its scopes; `missing` what its row asks for beyond them ([partial grants](pipes-browser-binding.md#partial-grants)); `note` the row's note; for a row taking several accounts, `accounts: [{id, account?, state, granted?, missing?, disconnecting?}]` oldest first and `accountsMax` ([several accounts](pipes-browser-binding.md#several-accounts)). Reading it tells their computer the states |
| `POST /api/connections/{provider}/authorize` | the shell's session (a signed key 403) | `{}`, or on a row of several accounts `{add: true}` (400 at `accounts_max`) or `{account: "<id>"}` (not theirs 404) → `{provider, url}` and a host-only state cookie. A connection's state, its session binding and S256 verifier are kept ten minutes; an own key's sign-in uses its computer (404 without one), `fragment_own_key` and PKCE. No `connection_oauth`: `not_connected`, no vendor call. Not offered 404; a provider with no sign-in 400 |
| `GET /api/connections/{provider}/callback?code=&state=` | the browser that began it, on the platform host, with its state cookie and the **same live session** | → HTML with no credentials: the code exchanged once and the grant imported into the person's Pipes account with exactly the scopes it holds (an own key's sealed by their computer); of several accounts, a grant of one already connected (its ID token's `sub`) reauthorizes it. A failure is 303 to `?refused=<word>`: `elsewhere`, `signed-out`, `declined`, `not-under-way`, `code-refused`, `no-scopes`, `one-account`, `account-limit`, `finishing`, `no-key`. Another host 404 |
| `DELETE /api/connections/{provider}[?account=<id>]` | a person | → `{provider, account?, state, revoked}`: a connection disconnected (of several, the one named; none named with several 400; `account` on a row of one 400; an id not theirs 404). The account is fenced first, so no connect begun before lands after (while one is importing: 429, asked again); its grant revoked at the row's `revoke` endpoint (RFC 7009) when it names one, then deleted at Pipes; the computer told. `revoked`: the provider answered 200. Another kind 400 |
| `PUT /api/connections/{provider}/key` | a person | `{key}` (1 to 4096 printable characters, no space) → `{provider, state: "set"}`: an own key, sealed by their computer (404 without one); another kind 400 |
| `DELETE /api/connections/{provider}/key` | a person | → `{provider, state: "not_set"}` |

## The shell

The platform's own page (cell/shell/, its files at `/__shell/<file>` from
the release's files). Its look and behaviour are its code's; what follows
is what other parties rely on.

| method & path (platform origin) | what |
| --- | --- |
| `GET /`, `/settings`, `/apps`, `/history`, `/welcome` | the shell's page, for anyone; signed out it offers Sign Up and Sign In (`/auth/login?screen_hint=…`, back to the same address). It reads `/welcome?trial=<code>`, `/welcome?checkout=<id>\|canceled`, `/settings?pack=<id>\|canceled` (Billing) and `/?chat=<name>` |
| `GET /about`, `/privacy`, `/terms` | static pages, for anyone |
| `GET /admin` | the operators' page (Operators: the admin) |
| `GET /share/<name>` | the share sheet (Sharing) |
| `GET /__shell/<file>` | the shell's files; asked for by version (`?v=<hash>`) `immutable`, else revalidated |

**The shell's session.** A request on the platform's host is taken as the
signed-in person when it carries `x-fragment-shell: 1` and
`Sec-Fetch-Site: same-origin`, and, writing, the platform's exact
`Origin`. `GET /api/fragments/watch`, which can carry no header, is taken
by that `Origin` alone, and an export's download by its Fetch Metadata
alone (Export). Anything else needs a signature; adding a key still needs
a key.

**Messages with framed pages.** The shell frames a person's fragments
(chats, app windows) and their computer's ports (an agent's screen, its
SimpleX link page). Their messages, each `{fragment: <kind>, ...}`, are
defined in one module, templates/chat/site/frame-messages.js (the shell's
cell/shell/frame-messages.js links to it): every kind, who sends it to
whom, its fields and what it does. The shell takes a message only from a
frame it made, at that frame's own origin, and answers that frame alone;
a page's messages ask, never grant. Framed pages take messages only from
their parent at the platform's origin (the `__live` hello's `platform`).

### A new release

The shell's page names the release that answered it (`<meta
name="fragment-release">`, `FRAGMENT_DEPLOY_ID`, and
`fragment-release-deployed`, `FRAGMENT_DEPLOYED_AT`), and every answer of
the platform's host names its own (`x-fragment-deploy`). Once an answer
names another, the page reloads where it was, its frames with it, while it
is hidden or at the person's next move, never while a chat says it is
`busy` or the shell holds unsent work. A chat keeps its composer's draft in
its own origin's storage as its person's (`{who, text, at}`), so a reload
loses nothing typed.

### A chat's alerts are the shell's

A browser lets no cross-origin frame ask for notifications, so a framed
chat's bell asks the shell (`alerts?`, `alerts`), which answers
`{fragment: "alerts", nonce, state: on | off | denied | unsupported |
failed, subscription?: {endpoint, p256dh, auth}, endpoint?, error?}`.
Turning on is the release's chat's alone (its `code.id` is
`blessed:chat@…`): a fork is its author's code, which could push anything
in the platform's name; any chat may read and turn off what it had. On
subscribes the shell's alerts worker (`/__shell/alerts-sw.js`, push only)
at a scope of that person's and chat's own
(`/__shell/alerts/<person>/<chat>/`) with the chat's VAPID key; the chat
stores the subscription on its fragment as its person (`__push-sub`), so
only the chat's fragment can push to it. On each push the worker asks who
is signed in and what the chat runs, and shows it only while its person is
signed in and the chat runs the release's chat; otherwise it shows nothing
and lets itself go. The shell's Sign out and Use another account let every
registration go first.

## Computers (docs/computers.md)

A person's computer runs an image and the agent fragments assigned to it;
the Computer Durable Object (`cell/src/computer.rs`) alone talks to its
container. What a guest may rely on, and every behaviour below, is
docs/computers.md's; these routes are its owner's. A view is
`ComputerView` (`fragment_proto::computer`).

| method & path | who | body → answer |
| --- | --- | --- |
| `POST /api/computers` | a person | → `ComputerView`: theirs, made asleep on `FRAGMENT_COMPUTER_IMAGE`; again, the same one. When the registry does not take its ask for its seat's push, 503 `registry_unavailable`: make it again |
| `GET /api/computers` | a person | → `{computers: [ComputerView], support}`, `support` `FRAGMENT_SUPPORT_URL` or `null` |
| `GET /api/computers/{id}` | its owner (anyone else 404) | → `ComputerView`. Their pages are told when it changes (`watch`'s `changed`) |
| `POST /api/computers/{id}/wake` | its owner | → the view once awake; 503 `wont_wake` when it would not start |
| `POST /api/computers/{id}/sleep` | its owner | → the view once asleep, or awake with its `why` when its save failed; within the sum of its steps' bounds (`SLEEP_STEPS_MS`), at most 11 minutes |
| `POST /api/computers/{id}/restart` | its owner | `{generation?}` → the view once started again, fresh from the image and its newest good save. `generation` names the start the owner saw: one already restarted is not restarted again. At zero credit 402, a guest 403 |
| `POST /api/computers/{id}/notices/seen` | its owner | `{life}` → the view, that `went_back` notice told no more; a `life` past the newest, or 0, 400 |
| `POST /api/computers/{id}/notices/later` | its owner | `{version}` → the view, that update put off; not its next start's, or not 12 hex, 400 |
| `PUT /api/computers/{id}/image` | its owner | `{image}` → the view: its next start's image; one the deployment lacks 400 |
| `PUT /api/computers/{id}/agents/{fragment}` | the owner of both | → the view: the agent fragment runs on it, its own key the agent's identity, an editor of its own fragment. Again, nothing changes |
| `DELETE /api/computers/{id}/agents/{fragment}` | the same | → the view: it signs nothing for the guest from now on. One gone or not on it: the view, unchanged |
| `PUT /api/computers/{id}/agents/{fragment}/accounts/{provider}` | its owner | `{account: "<id>" \| null}` → the view: which of the owner's accounts at a several-account connection the agent uses (`null`, the oldest connected). Not theirs 404; another provider, or no `account`, 400 |
| `PUT /api/computers/{id}/agents/{fragment}/connections` | its owner | `{connections: [provider] \| null}` → the view: the providers the agent may have swapped in; `null` every one its owner has (decision 44). One not offered, or none named, 400 |
| `POST /api/computers/{id}/agents/{fragment}/door` | its owner | `{door, act: "link" \| "unlink"}` → a ticket (An agent's doors). `door` a connector's id (else 400; `simplex` today); a link of one the deployment does not allow 403. Anyone else 404; an agent not on this computer 403 |
| `GET /api/computers/{id}/uses` | its owner | → `ComputerUses {computer, month, uses: [{provider, agent, calls, micros}]}`: this UTC month's calls through its swap a provider answered (under 500), and their charge (an operator key's; others 0). Thirteen months kept |
| `GET /api/computers/{id}/uses/{YYYY-MM}` | its owner | → the same for that month; not a month 400 |
| `POST /api/computers/{id}/ports/{port}/ticket` | its owner | `{path?}` → `{url, expiresAt}`: a one-time link (two minutes) signing a browser in to the computer's origin, `<24 hex>--computer.<suffix>` (`/__ticket`, then `/p/<port><path>`). `path` (default `/`) starts with `/`, at most 512 visible characters, with no `//`, `#`, `\`, `.` or `..` segment, else 400 |

On a computer's origin `/__ticket?t=` redeemed by a top-level visit sets
`fragment_computer` (HttpOnly, SameSite=Lax), by a frame's navigation
`fragment_computer_frame` (`HttpOnly; Secure; SameSite=None;
Partitioned`); both `__Host-` over https, for 12 hours, naming its owner
only. Every answer there but a socket's upgrade carries `frame-ancestors
<platform>`. Its cookies count as on a fragment's origin (Which cookies
count); a socket from any page but its own is 403.

A guest's request to a provider's host has its placeholders swapped
(docs/computers.md); the swap's refusals reach the guest as the
platform's, reaching no provider: 400 `invalid_request` (a malformed
placeholder, a provider not offered or in a place it does not take, more
than 4, or two agents' in one request), 403 `forbidden` (another host than
its provider's, a tag naming no agent here, an agent narrowed from it), 403
`not_connected`, or the ledger's 402 or 403.

### An agent's routines (decision 38)

An agent's routines are scheduled records on its own agent fragment's
`tasks` (Scheduled records), which it adds as its fragment's editor. Each
fire wakes its computer, and the bridge runs it as a turn in its chat, or,
naming none, in its owner's home (docs/chat-records.md).

### An agent's doors (decision 32)

A door is a way an agent's owner reaches it other than its chat: SimpleX,
run by its computer (docs/bridge.md, "SimpleX"), or a hosted door, a
connector fragment of its owner's (below). A door's id is a connector's
(`fragment_core::connectors::id_ok`), and the deployment's `connectors`
(`FRAGMENT_CONNECTORS`) say which may be opened, asked again at each
ticket, redemption, relayed record and hosted publish or fetch: a door the
deployment stops allowing is refused at once, and can still be unlinked.
What comes through one speaks for its owner, so only the owner opens one,
and the agent itself, whose tools act with its key, can neither open one
nor speak through one (`fragment_core::door`):

- **A ticket** starts a link or an unlink, made only by its owner (the
  route above): `{door, act, agent, computer, nonce, exp, sig}`, `sig` the
  agent fragment's key's BIP-340 signature of the SHA-256 of `fragment
  door ticket 1\n<door>\n<act>\n<agent>\n<computer>\n<nonce>\n<exp>`, for
  ten minutes. The computer's bridge checks it with that key and takes each
  once. An unlink's closes the door as it is made.
- **A link's ticket is redeemed once**, by the computer's guest: `POST
  /api/computer/door {agent, door, nonce, key}` (docs/computers.md)
  registers `key` (64 hex), the door's public key. Only the link ticket made
  last, within its ten minutes, is taken; anything else is 403. An agent
  has one such door: a link redeemed replaces the one open.
- **A relayed record** (`relayed` in its body: docs/chat-records.md, "A
  relayed message") `{via, for, agent, at, sig}` is taken only from an
  agent acting as itself for its owner (`for`), through the door its
  fragment (`agent`) holds open (`via`), `sig` the door key's BIP-340
  signature of the SHA-256 of `fragment door record
  1\n<via>\n<fragment>\n<channel>\n<post id>\n` followed by the body as
  canonical JSON (keys in order, no space) less `relayed.sig`, within ten
  minutes of `at` (ms). Anything else saying `relayed` is 403; an app's
  operation or job publishes none (400), but a hosted door's.
- **A hosted door** is a fragment made from a blessed connector template
  (`blessed::CONNECTORS`: `telegram`, templates/telegram) that has never
  run other code (another install unbinds it for good and removes its
  secrets, `door.unmade`). Only its owner
  edits or shares it: anyone else, an agent for its owner included, is at
  most a viewer there, an editor's role for anyone is 400, and its status
  shows an agent no share link. Only its owner, in person, binds it (`POST
  /api/f/{name}/door`, or `__door`), to an agent of theirs, which joins as
  a viewer. Bound and allowed, its app may publish records in its own
  channels whose `relayed` is exactly `{via: <door>, for: <owner>, agent}`,
  unsigned, and its jobs read that in `job.door`. The binding is the
  connector's, so several connectors may each be a door of one agent; an
  unlink leaves the agent a viewer. Its rules are cell/src/doors.rs's.

A SimpleX door's key and ticket live in the agent fragment's `meta`, and
go with it.

### An agent's home

Where an agent reaches its owner when no one asked:

- Its `agent.json`'s `home`: `"simplex"`, its SimpleX chat; absent or
  anything else, its own chat (`<label>-chat`). `"simplex"` with no SimpleX
  chat, or its link let go, is its own chat.
- **Its SimpleX chat** is its owner's chat labelled `<label>-simplex`,
  titled "SimpleX", the agent a member: the chat its SimpleX link mirrors,
  made by its computer as the link is first bound and kept through an
  unlink (docs/bridge.md, "SimpleX").
- A routine naming no chat runs in its home, read as it fires; its computer
  tells the agent where its home is at every turn (docs/computers.md).
- Its owner sets it in Settings, and through the SimpleX link page's
  ticket (The shell); an unlink sets it back. Each is written onto
  `agent.json` as it holds it, then deployed.

### A chat's records

A chat's `chat` and `work` records (turns, steps, replies, prompts, Stop)
are docs/chat-records.md's: the bridge writes them and the chat template
reads them.
