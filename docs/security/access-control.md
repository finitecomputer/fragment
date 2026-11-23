# Access control: who reaches whose data

For the CASA assessment (OWASP ASVS V4 access control and V8 data
protection; FIN-584). This lists every authenticated route, the id it
names, and where the platform checks that the caller may reach the thing
that id names. It was written from the code on 2026-11-23, with `master` at
`06e158bc7`, and corrected after a Codex review the same day, on `master`
at `9094a72c5` (where `access.rs` became `permit.rs`, FIN-615). When a
route changes, change its row here.

## The short answer

No route trusts an id the client sends. Each request reaches its data one
of three ways:

1. **By the signer alone.** The router verifies who signed (a NIP-98
   signature or the shell's session: `cell/src/lib.rs` `signer`, `caller`),
   then derives the object from that identity: their list, ledger, Vault,
   default computer, export. An id in the path picks among *their own*
   things, and someone else's is not found.
2. **By a fragment's name, checked by the fragment.** `/api/f/{name}/…`
   is handed to that fragment's Durable Object with the verified signer in
   headers only the router sets (`cell/src/routed.rs`). The router drops a
   client's copies of those headers, and the `lockdown` e2e section checks
   that. The fragment reads the caller's membership from its own `members`
   table and decides with one pure function,
   `fragment_core::permit::decide` (`crates/core/src/permit.rs`), before
   the route reads or writes anything. Two things happen to every routed
   request before that check, refused or not (`cell/src/fragment.rs`
   `route`): it counts once on the owner's request meter (`meter.rs`
   `count_request`), and the first one of an activation from a signer is
   offered to the fragment's `woke` trigger (`jobs.rs` `woke_chance`),
   which on `master` starts its catch-up job for any signer, a refused one
   included (FIN-660; the fix asks that the signer may read the
   fragment). The job runs the owner's own code with no input from the
   request, and only when something is due.
3. **By an org or offer id, checked by the registry.** The registry reads
   the caller's own org membership and refuses an id from another org
   (`cell/src/registry/seats.rs` `member_in`, `content.rs` `offer`,
   `orgs.rs` `org_of`).

The routes keyed by a person refuse agents where the data is the person's
alone: computers, connections, Vault, export, notices and the access view.
An agent acts for its owner only (`permit::may_act_for`, decision R17), and
never above editor (`AGENT_ROLE_MAX`).

**Three defects were found** in review, each filed and fixed in its own
PR: a scheduled record showed its body to viewers who may not read its
channel (FIN-655); a subscription kept delivering after its subscriber's
read access narrowed (FIN-656); and a refused signer's request could
start a fragment's `woke` job (FIN-660). Every other route in the tables
below checks ownership or membership before it reads or writes. The
other gaps are under "Gaps and observations".

## Fragment roles

A fragment has members at a role: `owner` (one), `editor` or `viewer`.
Everyone else reaches it by its visibility: `members` (nobody else),
`link` (a viewer while holding the share link), or `public` (the `public`
role, plus a viewer with the link). A person whose own agent is a member
reads, but never acts, as a viewer. An agent held below its owner acts
with at most its hold, for its owner, or as itself where its person is a
guest. On a connector everyone but its owner, in person, is at most a
viewer. `decide` answers 401 to someone unsigned and 403 to a signer,
never the data.

The share link (`?view=<token>`) counts only on the fragment's own site
(`cell/src/serve.rs` `answer`, compared in constant time with `eq_ct`). It
lends nothing on `/api/f/…`: every API route decides with `link: false`.
It moves into a cookie on its first navigation and off the URL, so the
page's code never reads it. The owner's `POST rotate` replaces it. The
`idor` section holds a real link on a `link` fragment to both: its site
answers an unsigned holder, and its control API answers the same token
403 to a signed stranger and 401 to no one signed.

## Fragment routes (`/api/f/{name}/…`)

Resource: the fragment `{name}`, a Durable Object of its own. Its blobs
are under its own npub in R2 (`blob_key`), so another fragment's hash is
not found there. Checked in `cell/src/fragment.rs` `route`, through each
handler's first line (`require`, `admit` or `decide`). "Owner" means the
fragment's owner, in person, or their own agent sharing for them where a
row says so. Tests: the pure rule's exhaustive test,
`permit::tests::no_one_reaches_more_than_their_own_standing_gives`,
which compares `decide` with a model of the rules (caps, holds, an
agent beside its person and a connector included) for every standing, and the `idor` e2e section
(`crates/e2e/src/lanes/idor.rs`, not yet run), which asks the reads,
the secrets, files, blob upload, deploy, schedule, card and icon writes,
and the owner's routes (membership changes, invites, visibility, rotate,
delete) as a stranger, a viewer and an editor, against objects the owner
made, and the owner's subscription by id. It does not ask an
operation's call or a channel post (they need app code and declared
channels: the `ops` and `posts` sections do).

| Method and path | Needs | Checked at |
|---|---|---|
| `GET status`, `manifest`, `members`, `files`, `file?path=`, `events`, `channels`, `triggers`, `schedules`, `card`, `icon`, `address`, `search/status` | viewer; each schedule only to a role that may read its channel (FIN-655) | `fragment.rs` `status`; `plane.rs` `manifest`, `files`, `file`; `members.rs` `members`; `channels.rs` `channels`; `jobs.rs` `triggers_api`; `schedules.rs` `schedule_list`; `card.rs` `card_api`; `icon.rs` `icon_get`; `address.rs`; `searchable.rs` |
| `GET channels/{c}`, `channels/{c}/newest` | the channel's read role (viewer for built-ins) | `channels.rs` `channel`, `newest_api` |
| `GET runs`, `runs/{id}` | viewer | `jobs.rs` `runs_api`, `run_api` |
| `GET`/`HEAD blobs/{sha}` | viewer | `blobs.rs` `get_blob` |
| `GET storage-token?access=read` / no `access` | viewer / editor | `plane.rs` `storage_token`; the repo itself only for members (`permit::Action::Repo`) |
| `POST search` | viewer | `searchable.rs` `search_found` |
| `GET secrets`, `PUT`/`DELETE secrets/{key}` | editor | `members.rs` `list_secrets`, `put_secret`, `delete_secret` |
| `POST files`, `deploy`, `refresh` | editor | `publish.rs` `write_files_api`, `deploy_api`; `plane.rs` `refresh` |
| `PUT blobs/{sha}` | editor | `blobs.rs` `store_blob`; the router checks the declared length first (`lib.rs` `blob_length`) |
| `POST card`, `PUT card`, `PUT face`, `PUT`/`DELETE icon`, `POST icon/make` | editor | `card.rs`; `members.rs` `face_put`; `icon.rs` |
| `POST replay`, `pause`, `schedules`; `PUT`/`DELETE schedules/{id}` | editor | `jobs.rs`; `schedules.rs` |
| `POST search/upsert`, `search/delete`; `GET search/sources` | editor | `searchable.rs` |
| `POST ops/{op}` | the operation's declared role. A mutation or job decides as acting, so reading through one's agent's membership lends nothing; an operation declared `public` on a public fragment is anyone's to call, a mutation or a job included, rate limited per caller (`RateLimited`) | `ops.rs` `call_op` |
| `POST channels/{c}`, `PUT channels/{c}/draft` | the channel's post role | `channels.rs` `post`, `admit_poster` |
| `PUT members/{who}`, `DELETE members/{who}` | owner, or their own agent acting for them with their authority (not held below them, not on a connector); a member may remove themselves | `members.rs` `sharer`, `set_member`, `remove_member`; rules in `permit::agent_shares`, `refuse_set_role`, `refuse_remove`; the router refuses other agents first (`permit::agent_may_ask`) |
| `PUT address` | a viewer who shares (the owner, or their agent as above) | `address.rs` `claim_address` (`shares`); reserved as sharing at the router |
| `GET`/`POST invites`, `DELETE invites/{email}` | owner, or their own agent sharing for them | `members.rs` `sharer` |
| `PUT visibility`, `POST rotate`, `GET`/`POST door`, `GET database`, `PUT cap` | owner | `members.rs`; `doors.rs`; `export.rs` `database`; `meter.rs` `put_cap` |
| `DELETE` (the fragment) | owner, in person | `fragment.rs` `delete`; an agent is refused at the router (`permit::reserved`, `agent_may_ask`: `Reserved::Dangerous`) |
| `POST subscriptions` | a member whose role may read the channel; each delivery asks again (FIN-656) | `subscriptions.rs` `subscriber`; `record_delivery` |
| `GET subscriptions`, `DELETE subscriptions/{id}` | any signer, no membership: scoped to the signer's own, the owner's all, so another's id removes nothing (`removed: 0`) | `subscriptions.rs` `subscriptions`, `unsubscribe` (`WHERE principal = ? OR owner`) |
| `POST inbox` | its inbox token or its code's webhook secret, no role | `fragment.rs` `inbox` |
| `POST session` | the person's approved key | `auth.rs` `key_session` |

The site (`https://{name}.<suffix>/…`, its pages, `__op/`, `__live`,
`__blob/`, `__file`, `__search`) runs the same `decide` with the share
link counted (`serve.rs`). Each fragment is its own browser origin, on a
site apart from the platform's (decision 5). The `isolation` and `frames`
e2e sections test which cookies count there.

## Person-keyed routes

| Route | Resource named by | Checked at | Tests |
|---|---|---|---|
| `POST /api/fragments` (create) | the signer, who owns what they make. An agent's signature, with or without `for`, makes one for the agent's own owner: `create_fragment` takes the owner from the agent's identity (its registered owner), never from the request, so an agent signing bare reaches it too and still makes its owner's; a `for` naming anyone but its owner is refused first (`signer_for`) | `lib.rs` `signer_for`, then `create_fragment` (the maker is the agent's `owner`), then the new fragment's `create` | `create` |
| `GET /api/fragments/watch` (WebSocket) | the signer's own list: signed, or the platform's own page with its session (Origin checked) | `lib.rs` `watch_list` | `shell` |
| `GET /api/fragments`, `PUT /api/fragments/{name}/archived`, `dock`, `pinned`, `seen`, `GET /api/search` | the signer's own `Principal` object; `{name}` picks a row of their list | `lib.rs` (object keyed by `who.identity.id`) | `idor` (another's archive changes nothing of theirs) |
| `POST /api/search` (across fragments) | each named fragment, asked as the signer | `across.rs` → each fragment's `search_asked` (`admit` viewer) | `idor`, `search-across` |
| `/api/computers`, `/api/computers/{id}/…` | the computer `{id}` | `computer.rs` `ComputerCell::api` → `owned_by` (404 unless its stored owner is the signer); people only | `computers` (a stranger's restart of another's computer is 404, `RECOVERY_CHECKS`) |
| `/api/connections`, `/{provider}/authorize`, `DELETE /{provider}?account=` | the signer's WorkOS user; an account id must be one of theirs | `connections.rs` `route`, `going`; registry `connection_signin.rs` `disconnected` (by the signer's subject) | `connections` |
| `PUT`/`DELETE /api/connections/{provider}/key` | the signer's own Vault (an own key for a provider) | `connections.rs` → `vault::own_put` keyed by the signer | `connections` |
| `GET /api/connections/{provider}/callback` | the state cookie of the browser that began it, and its session | `connections.rs` `callback`, `auth::trip_bound` | `connections` (PKCE, another attempt's code) |
| `/api/vault`, `/api/vault/{name}` | the signer's own `Vault` object | `vault.rs` `route`; people only | `vault` (another person's list, delete by name) |
| `/api/export`, `/api/export/{id}/download` | the signer's own `Principal`; `{id}` among their exports | `export.rs` `route`, `downloader`; people only | `export` (anyone else's session downloads nothing, 404) |
| `/api/ledger`, `/api/ledger/uses`, `POST /api/ledger/quote` | the signer's payer (an agent's: its owner) | `lib.rs` `ledger_route`, `payer_of` | `ledger` |
| `POST /api/ledger/{who}/{command}` | any ledger | operators only (`orgs::operator`) | `ledger`, `orgs` |
| `POST /api/identities` (an agent's registration) | the owner's signature, and a proof signed by the agent's new key; a person is refused (people sign in) | `lib.rs` `identities` (`authenticate`, `proven_key`), registry `RegisterAgent` | `identities` |
| `PUT /api/identities/me/picture` | the signer's own identity | `lib.rs` `identities` (`View` then `SetPicture` by the signer) | `signin`, `shell` |
| `/api/identities/{id}`, `/{id}/keys`, `/{id}/keys/{k}`, `/{id}/held` | identity `{id}` | registry `view_for` (`may_view`), `managed` (`may_manage_keys`), `check` (`may_check_key`), `hold` (its owner only); rules in `crates/core/src/registry.rs` | `identities` ("nobody else adds, revokes, sees") |
| `GET /api/identities/{id}/picture` | public by design: a face | `lib.rs` `picture` | `faces` |
| `/api/invites`, `/api/invites/{name}/accept\|decline` | an invite to one of the signer's verified emails | registry `invites.rs` `answer_invite` (`invite_to` by the signer's id) | `idor` (another's invite is 404), `members` |
| `GET /api/access`, `GET`/`POST /share/{name}` | the fragments the signer owns; the sheet acts through the fragment's own handlers | `share.rs` `access`, sheet with Origin and form-token checks | `share` |
| `GET /api/notices` | the signer's ledger, computer, registry rows | `notices.rs` | `computers`, `orgs` |
| `/api/models/v1/…` | paid by the signer's payer | `models.rs` `caller` | `ledger`, `deliver` |
| `/api/migrations/stages/{stage}/…` | the stage, read as the signing agent | the `Migrations` object (`readable/{stage}/{agent}`); `copy` writes through the target fragment's editor check | `moves` |
| `/api/migrations/offers`, `/api/migrations/offers/{old}/{op}` | the signer, a person (an agent is 403); the `Migrations` object answers only an offer that is theirs | `migrations.rs` `offer_route` | `move` |
| `/api/migrations/mover/…` | the mover's key only | `migrations.rs` (`authenticate` against the configured mover) | `migrations` |

## Orgs and billing

| Route | Resource named by | Checked at | Tests |
|---|---|---|---|
| `GET`/`PUT /api/seat`, `POST /api/seat/offers/{id}/accept\|decline`, `POST /api/seat/leave` | the signer's seat; an offer to one of their emails | registry `offers.rs` `offer_for` (`emails_of(who)`) | `orgs` |
| `/api/org`, `/api/org/seats/{seat}`, `/api/org/admins/{member}` | the org the signer administers; a seat or member id must be in it | registry `seats.rs` `admin_of`, `member_in` | `orgs` |
| `GET /api/orgs/offers` | the content-admin offers to the signer's verified emails | registry `OffersToMe` (by the signer) | `governance` |
| `/api/orgs/{org}/ledger`, `content`, `content-admin-offers` (make, withdraw), `content-admins/{person}` | `{org}`; refused unless the signer administers it (its billing admins, or its content admins where the route is content's) | registry `orgs.rs` `org_of` (named org must be theirs), `content.rs` `governor`, `place`, `offer(org, id)` | `orgs`, `governance` |
| `POST /api/orgs/{org}/content-admin-offers/{offer}/accept\|decline` | the offer's recipient, before they are an admin: a person who signs in with the offer's email | registry `content.rs` `answer_offer` (the signer's `subjects` row must hold the offer's email, else 404) | `governance` |
| `POST /api/billing/checkout` | a person who signs in with an email: in no org, a Checkout for a new org of one, made once paid; an admin of an org that pays for nothing, in that org | registry `billing.rs` `checkout_begin` | `billing` |
| `POST /api/billing/packs`, `portal` | the org the signer administers | registry `PackBegin`, `CustomerOf` | `billing` |
| `POST /api/billing/sessions/{id}` | a Stripe Checkout whose metadata names the signer and this deployment | `billing.rs` `returned` | `billing` (another's Checkout is 404) |
| `POST /api/stripe/webhook` | Stripe's signature | `billing.rs` `webhook` | `billing` |

## Operators

`/admin`, `/api/admin/…` (the admin Worker, `cell/src/admin.rs`) and the
ledger commands need the deployment's operators' key or session
(`orgs::operator`, `session_operator`); anyone else gets the same 404 as
a missing route. `GET`/`POST /api/people/{person}/wipe` needs a request
signed by an operator's key (a key the deployment lists, or the key of
an identity it lists; a platform session never is one): an unsigned
request is 401 and a signed non-operator 403 (`wipe.rs` `operator`). `/api/test/…` exists only on a
fleet with a test secret, for a request that carries it, and is 404
otherwise (`lib.rs`, `levers::route`).

## Gaps and observations

- **Not run here.** The `idor` e2e section was written and compiled, but
  not run: this pass used no Docker, workerd or e2e lanes. Run it with
  `cargo xtask e2e --only idor` before counting on it. The other sections
  named above already ran on `master`.
- **A refused request still has effects before its check.** Every
  request the router hands a fragment runs three steps before its route
  decides who may ask (`fragment.rs`, before `route`):
  - It counts once on the owner's request meter (`meter.rs`
    `count_request`).
  - On a fragment of a blessed template installed from an older release
    (the platform deployed since), it installs the release's code again
    (`plane.rs` `blessed_current`, `install_code`), and that
    install fires the template's `installed` trigger (`jobs.rs` `fire_installed`),
    a job of the owner's: Brain declares one (`templates/brain/fragment.json`,
    `{"installed": true, "run": "changed"}`). So a stranger's refused
    request, the first after a deploy, can start the owner's reinstall and
    its job. It is the same work the owner's next request would start, once
    per release, but a stranger can choose when. Filed as FIN-698.
  - It may start a due `woke` job, for a signer the fragment would answer
    (FIN-660).
- **A fragment's existence shows.** `/api/f/{name}/…` answers 403 to a
  signer for a fragment that exists and isn't theirs, and 404 for one that
  doesn't. A search across says `denied` or `missing` the same way.
  Nothing of the fragment is answered. A name carries a random suffix
  (`fragment_name`: four characters of 32, 20 bits per label). Nothing
  limits how fast one signer probes names, so existence is guessable for a
  known label. Filed as FIN-649.
- **The body is read before the role.** Some fragment routes parse their
  JSON body before the handler checks the role, so a stranger with a
  malformed body gets 400, not 403. That says only that the body is
  malformed, never anything of the fragment.
- **Org, connection and billing ids** are covered by the registry checks
  above and by their own sections. The `idor` section doesn't repeat them,
  because they need Stripe and WorkOS fakes set up as those sections do.
