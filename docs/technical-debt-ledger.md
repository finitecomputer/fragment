# Technical debt ledger

Shortcuts are allowed only through this ledger (engineering style §1).
Each entry names where it was observed, the risk, the first proof that
would show it biting, and the condition that deletes it. An entry
without a delete condition is unfinished design, not debt.

## Two imports into one session archive at once agree last-writer-wins

- **Observed:** `fragment session import` (templates/sessions/README.md),
  2026-10-30: each conversation's file goes up as an upsert on whatever
  head main has, and only the indexes' commit is made again on a head
  another writer moved.
- **Shortcut:** two imports of the same conversation at once (the same
  person running it twice, on two machines) keep the last one's file,
  and an index entry may name the other's copy (its `sha256` and
  `ended_at`) when the two are the same size; one of another size is
  listed by its file at the next import (`found`, cli/src/session).
- **Risk:** a stale entry makes the next import of that conversation
  compare against the wrong copy: it may skip a newer copy once, or write
  the same one again. Nothing is lost: the file is one of the two copies.
- **First proof:** an archive whose entry's `sha256` is not its file's.
- **Delete when:** each batch's commit is made again from main as the
  indexes' is (read the entries of the conversations it writes, keep the
  newer), or the archive moves to an app whose mutation writes both.

## A paid org pack is still credited, though none is sold

- **Observed:** #736 review, 2026-10-30: orgs now pay out of band and an
  operator credits them (decision 66), so no org pack is opened any more.
  But preview sold one in Stripe's sandbox from #716 until #736 deployed.
- **Shortcut:** `pack_paid` (`cell/src/billing.rs`) still grants a
  Checkout whose `fragment_payer` names its org's ledger, once by its
  Checkout, so one paid after that deploy (its return, or a webhook
  Stripe sends again) is credited and not lost.
- **Risk:** none beyond the read: nothing opens such a Checkout.
- **First proof:** a webhook for such a Checkout that fails its grant
  (Stripe's dashboard shows it retried).
- **Delete when:** three days (Stripe's retries) have passed since #736
  deployed to preview; a Checkout itself expires after 31 minutes.

## Hermes' explicit auxiliary vision overrides native image routing

- **Observed:** #505 and the decision 65 probe (2026-10-29): all four
  deployed Fast/Deep models read user and tool-result images, but Hermes
  v0.21.7's explicit auxiliary backend forces text descriptions.
- **Stand-in:** `fragment-native-images` gives the routing decision a copy
  without that forced choice, retaining the real fallback backend. Its
  supported request middleware uses Hermes' resize helper on a temporary
  materialization, limits a call to four images of 256 KiB each, and edits
  only the API copy. Originals and canonical history remain intact.
- **Risk:** the decision function and resize/materialization helpers are
  pinned upstream APIs; a changed call path or middleware failure policy
  could bypass the bounds or restore unwanted descriptions.
- **First proof:** the real-Hermes native-image Docker test covers capable
  and text-only models, attachments, desktop captures, large/corrupt
  images, per-call bounds and retry; the delegation gates retain result
  delivery and wakefulness.
- **Delete when:** upstream supports an auxiliary fallback without forcing
  text, and one per-call image-count/encoded-byte budget for attachments
  and screenshots passes those same checks under the route's 6 MiB cap.

## The Hermes image adapts model preflight deployment waits

- **Observed:** #625, 2026-10-29: release guard refusals lasted about five
  minutes. A silent cell wait exceeds Hermes' Fast 20 s and planned Deep
  180 s stale limits, permitting abandoned calls to overlap.
- **Shortcut:** the image-owned `deploy_wait.py`, installed by the
  `fragment-deploy-wait` plugin, wraps pinned Hermes v0.21.7's
  `_StreamingCall._open_chat_stream`, stale-kill and heartbeat hooks.
  Only the private intercept's marked not-dispatched preflight refusals
  get a six-minute budget inside one attempt. A plugin error hook makes
  that budget's expiry terminal, rather than another fallback/retry cycle.
- **Risk:** an upstream change to the request-client retry policy,
  watchdog, cancellation or writer fence could allow a duplicate dispatch
  or suppress a real stall. Non-streaming and auxiliary calls do not use
  this adapter and retain their existing failure behavior.
- **First proof:** real-Hermes Docker: 90 s of marked preflight refusals,
  one reply and one provider dispatch, no provider recovery notice;
  cancellation before admission dispatches nothing; an ordinary silent
  provider still triggers the existing ~40 s Fast fallback. Contract tests
  exercise markers, budget exhaustion, cancellation and supersession.
- **Delete when:** Hermes natively supports a bounded deployment-preflight
  wait with these timer, retry, cancellation and no-dispatch guarantees,
  and the same real-image tests pass without the adapter.

Entries whose subject went at the cut (computers, sandcastle, Hermes,
the desktop, chats; docs/cloudflare-v1.md, decision 33) went with it
and are at the tag `celld-final`. Entries about the hosted fleet (Fly,
the node image, its secrets) are the `celld` branch's, which runs
fragment.club until cutover (decisions 34–35).

## Hermes external skills need an image copy-on-write plugin

- **Observed:** p5's 2026-10-08 first-chat logs showed a fresh-read refusal,
  then a PermissionError when the automatic reviewer patched managed
  `generate-pdf-finite`; three background model requests took 26–33s each.
  Hermes v0.21.6 has one global review switch and writes external skills in
  place. Memory-triggered review also admits skill tools. Paul reversed the
  temporary global-off demo setting on 2026-10-09: memory review is wanted.
- **Shortcut:** memory and skill reviews are on. The image-owned
  `fragment-skill-fork` plugin uses the supported pre-tool hook to copy our
  external package to the active profile's writable `skills/` before writes,
  preserving fresh-read guards and normal validation. Managed releases do
  not overwrite local forks. No upstream source patch is needed.
- **Risk:** the plugin relies on Hermes' skill resolver/read-mark internals
  (`_find_skill`, `_resolve_supporting_file`, the background review's read
  marks: v0.21.6's, unchanged in v0.21.7); an upgrade needs its Docker
  promotion test. External batches must fork
  via a single write first. A failed patch can leave an unchanged local fork.
  Reviews still consume model calls under the shared 50 calls/min limit.
- **First proof:** the Docker lane scripts a background patch through the
  real plugin dispatcher, checks unread-patch refusal, complete local fork,
  writable patch, helper read guards, name shadowing and immutable baseline.
- **Delete when:** Hermes natively supports managed-aware copy-on-write
  with the same ownership and fresh-read guarantees; remove the plugin
  after that behavior passes the promotion test. Budget tuning is Paul's call.

## The agent's page derives the model an agent runs; the image does not say it

- **Observed:** Paul, 2026-10-20 (production, his agent Waffle): the
  agent's page said "cheap model" while its chat's `/model` named the real
  one. The page now names it by its id from their computer's view
  (`models`, and each agent's `modelProviders`) and the agent's
  `agent.json` (docs/computers.md, "An agent's own model").
- **Shortcut:** the rule that picks the model (its own while its provider
  is one it may run models of, else its tier's: `medium`, else `cheap`) is
  written twice: in our Hermes image (`hermes-boot`'s `MainModel::of` and
  `Tier::of`, which also refuse an `agent.json` model whose id is no id)
  and in the agent template's page (`runs`). Nothing the image chose
  reaches the platform.
- **Risk:** the two drift, or an image runs by a rule of its own (a high
  tier turned on, an id the image refuses), and the page names a model the
  agent does not run. The page reads `agent.json` as live has it
  (`__file`), the computer as main has it: Settings puts each pick live,
  but a change committed and not deployed (the agent's own, the CLI's)
  shows on the page only once it is.
- **First proof:** an agent whose `agent.json` names a model its image
  refuses while its provider is connected: the page names it, the chat's
  `/model` does not.
- **Delete when:** the image says what each agent runs (its view's agents
  written back, `PUT /api/computer/agents/{a}/model` or the like, as
  `profile.model` already logs it), its owner's view carries it, and the
  page shows that; an image change, which ships through a canary.

## OpenRouter's calls are charged at their ceiling, not their cost

- **Observed:** 2026-10-15, moving Finite's tiers to GLM-5.3 Flash
  through OpenRouter (decision 61). The ledger prices tokens from the
  book; OpenRouter routes each call to one of several providers, each at
  its own price under the slot's `max_price`, and reports what the call
  cost in its usage (`usage.cost`, in credits).
- **Shortcut:** an OpenRouter slot's model is priced at its routing's
  ceiling, read from the config (`max_price`: $0.15 in, $0.50 out per
  million on Finite's), cached and written tokens at the prompt's, with
  OpenRouter's 5.5% fee (fragment_core::models::Upstream::ceiling), so
  no call is charged below what OpenRouter could bill. `usage.cost` is
  not read.
- **Risk:** cached input is overcharged most: a cached token is charged
  at the prompt ceiling, $0.15 a million before fee and margin, where the
  endpoints bill $0.012 to $0.03 for one (about five times Workers AI's
  Flash's $0.03), and most of an agent's prompt is a cached prefix. A
  call of 30,000 tokens in (25,000 cached) and 600 out is charged about
  $0.0076, against $0.0028 on Workers AI's GLM-5.3 Flash; output is at
  most the call's cost (Novita's endpoint bills $0.28 a million, under the
  $0.50 charged).
- **First proof:** a person's month on the ledger against the platform's
  OpenRouter activity for the same calls (its generation ids), or a
  person asking why a turn costs more than it did on Workers AI.
- **Delete when:** a call is charged from OpenRouter's reported cost
  (a `Usage` of the vendor's cost in micro-dollars, the fee and margin
  on it): the first change after launch (the coordinator for Paul,
  2026-10-15).

## A ledger's head carries `holds` and a `book` for a rollback

- **Observed:** 2026-10-21 (decision 62; review of #474 and #475). A
  ledger keeps no holds and no price book now, but the releases before
  require both in a head, and every route writes its head back.
- **Shortcut:** a head is written with `holds` empty and the book of the
  release writing it (`fragment_core::ledger` `OldHolds`,
  `Ledger::written`), which no release since reads, and a head read with
  holds has them retired, their entries ended as released
  (`retire_holds`), so a rollback to a release before decision 62 reads
  every head this one wrote and finds no held entry.
- **Risk:** none in use: a few hundred bytes a head, written and never
  read. Kept past its need, it is dead weight that reads as if it meant
  something.
- **First proof:** n/a (a compatibility field, not a bug).
- **Delete when:** no release before decision 62 can be deployed again
  (production and the preview past it, and Paul agrees a rollback that
  far is not wanted), and no head holds any (every ledger read since):
  drop `OldHolds`, `retire_holds` and `written`'s book, and the
  `a_head_written_here_reads_in_the_release_before` and
  `a_rollback_finds_no_hold_and_no_held_entry` tests.

## A paid call's charge is best effort

- **Observed:** 2026-10-21 (decision 62; Paul: "billing should be
  settled best effort not interrupt every request"). A model call is
  gated by a cached standing and charged after its answer, as a row on
  the ledger's queue (cell/src/models.rs, meter.rs `charge`).
- **Shortcut:** a call that reports no usage (an upstream stream that
  broke before its last chunk, an answer with no `usage`) is charged
  nothing, said aloud (`ledger.unknown-usage`), where a hold charged its
  worst case; a charge whose queue send fails is lost, said aloud
  (`ledger.charge-lost`); and a person out of credit spends on for up to
  the gate's minute in each isolate.
- **Risk:** calls given away: a provider whose streams break before
  their usage, or a queue outage, costs the platform what those calls
  cost; an agent at zero runs on for up to a minute. Cents, against an
  agent never stopped by billing.
- **First proof:** `ledger.unknown-usage` or `ledger.charge-lost` lines
  in the Worker's logs, more than a few a day.
- **Delete when:** a call with no usage is charged from what it streamed
  (its output's tokens counted, its input's bytes), and a failed send is
  kept and sent again.

## Dev's cell is not the release's build

- **Observed:** 2026-10-24. A change to one of the release's files (a
  CSS rule in cell/shell) moves the index the cell carries, so the cell
  recompiled to wasm32 under fat LTO and wasm-opt: about 115 s on an M-series
  Mac per edit under `cargo xtask dev`.
- **Shortcut:** `cargo xtask dev` builds the cell in cell/Cargo.toml's
  `devstack` profile (the release's less fat LTO and one codegen unit,
  incremental) without wasm-opt (`cargo xtask build --dev`,
  xtask/src/build.rs `Profile`): about 13 s for such an edit, 8 of them the cell's. Same
  source, same flags, same files, same router and asset code; its Wasm
  is about 7.7 MB and held to no budget. Deploys and the e2e build the
  release's and refuse dev's (crates/devstack/src/built.rs).
- **Risk:** what only the release's code generation shows (a size
  regression, a miscompile under LTO, wasm-opt's rewriting, a slower
  cold start) is not seen in dev; it is seen by `cargo xtask e2e`, CI
  and the preview, which run the release's.
- **First proof:** a behaviour the e2e or the preview shows that the
  same tree under `cargo xtask dev` does not.
- **Delete when:** the release's build rebuilds a change to one of its
  files in seconds (its index supplied without recompiling the cell),
  and dev builds the release's again.

## The cell's Wasm budget was raised for launch

- **Observed:** 2026-10-14. The cell diet (#305) set the budget at
  6.00 MB raw; by launch week Export everything (#315, about 400 KB) and
  operation descriptions (#327) had it at 5.98 MB, and Finite Private's
  model routing (#330) took it 6 KB over, with decide and `ai.text`
  tools still to land.
- **Shortcut:** `CELL_WASM_MAX_BYTES` is 6.5 MB (xtask/src/build.rs), so
  the launch week's features land without squeezing.
- **2026-10-19:** master with #365 and #366 was 6.38 MB, and #367 took it
  18 KB over. #367 made the registry's and Stripe's round trips
  non-generic (`ask_registry` and the Stripe client parse their answer in
  the one generic step, so a call type no longer copies a whole async
  round trip): 6.34 MB with #367's 134 KB.
- **Risk:** every isolate copies the Wasm's data into its memory and
  every deploy uploads it: a bigger cell starts slower and leaves less
  memory to requests, and a budget moved once is easier to move again.
- **First proof:** a cold-start or memory regression in the cell's
  Workers Logs after a deploy that grew it.
- **Delete when:** the cell is back under 6.0 MB, the first candidate
  being Export everything's archive builder moved into a Worker of its
  own, and the budget is set back to 6.0 MB.

## A Hermes release has no rollback

- **Observed:** 2026-10-13, designing the canary (docs/computers.md,
  "Stable and canary"). Pinning a canary computer back to stable starts
  the older image on the `/data` the canary wrote, its newest save. A
  computer keeps three saves, each turn's end replacing the oldest
  (`fragment_core::computer::Saves`), so after a few turns none is from
  before its image changed: decision 19's "point-in-time restore when the
  new image moved its data's schema" has nothing to restore.
- **Shortcut:** a round that changes only our layers on the same Hermes
  base, whose files our code reads either way, can be pinned back: that is
  a rollback. A Hermes release is forward only (Paul, 2026-10-13): a bad
  one is fixed by the next release, never pinned back. The first such
  round is v0.21.6 to v0.21.7 (2026-10-16).
- **Risk:** a Hermes release that breaks on real computers stays broken
  for the canary group until a fix is built; pinned back anyway, an older
  Hermes opens databases a newer one migrated, which it may refuse or
  misread (the image's `computer-check` runs `PRAGMA quick_check`, which
  a schema it does not know passes).
- **First proof:** a canary computer pinned back after a Hermes release
  whose Hermes logs a schema or migration error at boot, or whose sessions
  are missing.
- **Delete when:** a computer keeps the save an image change started from
  (the save its first start on a new image restored), apart from its three,
  until its next image change, and a pin back restores it: a rollback then
  loses only the canary's own work, and says so as any going back does.

## A direct chat's label is its agent's, which a branch may lack room for

- **Observed:** 2026-10-08, no usernames (decision 47) on the host label
  limit (docs/api.md, Names). A person's direct chat with an agent is
  labeled `<the agent's label>-chat`, by the shell and by `fragment ask`
  alike (`cli/src/ask.rs`, `direct_label`), so both find the same chat.
  An agent made by the CLI may have a label of 57 bytes (the shell's are
  at most 31). A pair chat's label is cut to the room every deployment
  leaves (`limits::LABEL_ROOM_MIN_BYTES`), so it fits anywhere.
- **Risk:** on a branch, an agent whose label comes within `-chat` of the
  room there (49 bytes or more on `p5`) is refused its direct chat (400,
  saying why), where on production it gets one. Never a fragment no host
  reaches.
- **First proof:** a direct chat's create on a preview answering "… is
  too long: a fragment's address …".
- **Delete when:** an agent's label is held, where it is made, to the
  room every deployment leaves less `-chat`, or a direct chat is found by
  its members rather than its label.

## A wipe finds a person's fragments through their lists

- **Observed:** 2026-10-07, the operator's wipe (docs/api.md, Operators).
  No index names every fragment a person owns: a wipe finds the ones a
  person owns on their and their agents' lists (each fragment's own
  outbox delivers its rows) and the agent fragments the registry names.
- **Risk:** a fragment whose create its owner's list never took (the
  delivery failing from the create on, and still at the wipe) is not
  ended, and outlives its owner, its repo with it; once their list is
  wiped, a late delivery is taken and kept nowhere.
- **First proof:** a wiped person's fragment answering after their wipe
  said nothing was left.
- **Delete when:** the registry (or a per-person index) records each
  fragment as it is made, and the wipe reads it beside the lists.

## An agent made later joins best effort

- **Observed:** 2026-10-23, decision 64 (`claude/member-with-agents`):
  a person's agents are members beside them where they accepted an
  invite. One made after their yes comes in as it is registered (a
  computer's, once its computer takes it: computer.rs): the
  platform reads its person's list and asks each fragment of someone
  else's they are in (`members::agent_made`), once, with no outbox: a
  fragment that does not answer then never hears it.
- **Risk:** a person in with only some of their agents (one made later
  missing), until their next yes there: it reaches the fragment acting
  for them (R17), but is in no chat's roster there, and its computer
  does not follow it.
- **First proof:** a log line `{"agent-made": …, "fragment": …, "failed":
  …}`; a person's agent missing from a fragment's members beside them.
- **Delete when:** an outbox carries it (the person's list, which knows
  both their fragments and when one is told, retrying on its alarm), or
  a fragment reconciles a person's agents as it is read.

## Invites from before their yes are met by a sign-in

- **Observed:** 2026-10-22, decision 63 (`claude/editor-invites`): an
  invite now waits for its person's yes in the shell, but those mailed
  before it said "sign in as … to open it" and keep no role in the
  registry (`invites_waiting.role` NULL). So a sign-in still meets them
  as it did (registry/invites.rs `meet_invites`), with the kept claims
  and the alarm's retries (`invite_claims`, `claims_alarm`), which only
  they use now, and `GET /api/invites` lists none of them.
- **Risk:** two ways into a fragment by email for a month: someone
  invited the day before the release joins at their sign-in without
  being asked. Each is one the owner made, as they made it.
- **First proof:** a sign-in after the release that makes its person a
  member (`member.joined`) of a fragment they never accepted, whose
  invite was made before it.
- **Delete when:** `INVITE_TTL_S` (30 days) has passed since the release
  that carries decision 63: every such row has run out. Then
  `meet_invites`, the kept claims, the alarm's claims and the `role IS
  NULL` clauses go, and `invite_claims` with them (a step that drops it).

## Objects made before a column of their table refuse what writes it

- **Observed:** 2026-10-08, p5: a wipe's cleanup never finished. #197
  (2026-10-07) removed every migration as state "from before
  Cloudflare", among them three made for Cloudflare-era columns: a list's
  `memberships.searched` (#186, 11 minutes before), a fragment's
  `runs.code` (#178, 9 minutes before) and `code_triggers.from_kind`
  (#117, 10-03). `CREATE TABLE IF NOT EXISTS` never adds a column, so an
  object made before each refuses every statement that names it (a 500):
  a list made before #186 takes no change (its fragments' rows freeze), a
  fragment made before #178 launches and advances no job run, one made
  before #117 installs no deploy and fires no channel trigger. Two more
  columns never had a migration (#116): the Registry's
  `identities.held` (a Registry made before it, one object for the whole
  deployment, refuses every sign-in and key) and a fragment's
  `members.people_only` (unread and unwritten since people-only shares
  went, 2026-10-13: its table keeps it, at its default, until the
  fragment is made again). A wipe no longer waits on its person's own lists
  (ended.rs), so this holds no wipe; everything else of such an object
  stays refused.
- **Since 2026-10-19** schemas are versioned steps (docs/MODEL.md,
  Schemas; `fragment_core::migrate`): a column is a step that reaches
  every object, `cargo xtask check` fails one added any other way (the
  schema lock), and an object from before versions is adopted only when
  its tables are step 1's. Such an object, made before one of those
  columns, now refuses every call as it starts (`schema_refused`, "reset
  required", naming its table), not only the statements that name the
  column; a delete or a wipe still ends it (a Registry aside), and a
  fragment's delete makes its object current again.
- **Risk:** p5's (and the e2e preview's) objects made before those
  columns stay refused until they are reset. A Registry so made refuses
  the whole deployment. A fragment its owner's frozen list never took is
  one a wipe does not find (the entry above).
- **First proof:** an object answering `schema_refused` "reset
  required" (its log line `{"schema": …, "refused": …}` names it), or
  before this deploy a list that refuses an index change with `no column
  named searched` (`POST /api/test/list {op: before-searched}` makes one).
- **Delete when:** the deployments that hold such objects are reset (or
  those objects deleted: Paul's call, a hard cut).

## Objects made before schema versions are adopted, a variant fixed

- **Observed:** 2026-10-19, versioned schemas (docs/MODEL.md, Schemas).
  Every object deployed before then holds tables and no `_schema`:
  `migrate::adopt` runs step 1 on it and compares what it then holds
  with what step 1 makes, text for text, and `Class::variants` names the
  one shape known to differ harmlessly (a fragment's
  `members.people_only`, from 2026-10-03 to 2026-10-13), fixed by
  dropping the column. Production (finite.chat) starts with none.
- **Risk:** none beyond its code: an object adopted is one step 1 makes
  exactly; one that is not is refused, never guessed at.
- **First proof:** an adoption's log line (`{"schema": …, "ran":
  "adopted at 1 …"}`), or the e2e's `migrations` section.
- **Delete when:** the deployments made before 2026-10-19 (the e2e
  preview, p5, preview.finite.build) are reset or retired: an idle object
  adopts only when it next starts, so nothing else says every one has.

## An app facet's operation ledger has no schema version

- **Observed:** 2026-10-19. The platform's table in each app facet's
  SQLite (`_fragment_ops`, cell/platform.mjs) is made with `CREATE TABLE
  IF NOT EXISTS` as the facet starts; the platform objects' steps
  (`fragment_core::migrate`) do not reach it, and the schema lock does
  not name it (an app facet's database is its author's).
- **Risk:** a column added to it reaches no facet made before, which
  then refuses every mutation that names it, as the platform objects'
  did before their steps.
- **First proof:** a mutation answering `no such column` from
  `_fragment_ops` after a deploy that changed it.
- **Delete when:** the facet's ledger takes versioned steps of its own,
  or its shape is held by a check as the platform objects' are.

## The browser half of web push is not driven by a test

- **Observed:** phase 2 slice F. The e2e proves the server half end to end
  (subscriptions, VAPID, RFC 8291 encryption decrypted by a fake push
  service, the queue, retries, drops), and the encryption agrees with the
  old runtime's (checked once by hand). `fragment.push.register` and
  `__sw.js` run only in a real browser with a real push service. The
  chat's push (docs/chat-records.md, Push) is the same: its chat lane
  subscribes the push fake by `__push-sub`, as the page's bell would, and
  the shell's alerts (docs/api.md, The shell) subscribe through a fake of
  the browser's `PushManager` that hands over the push fake's
  subscription, and `alerts-sw.js` takes pushes DevTools delivers; its
  click is not driven.
- **Risk:** a page's subscribe or the service worker's display breaks
  unnoticed.
- **First proof:** subscribing from a phone or desktop browser to a
  hosted fragment.
- **Delete when:** a manual check on a hosted fleet (phase 3) is recorded,
  or a browser test can run with a local push service.

## A chat the shell frames is "looked at" while the shell is

- **Observed:** 2026-10-03, the chat's push (docs/chat-records.md, Push).
  A chat's page says it is `looking` while its document is visible, so a
  chat open in a tab of the shell that is not shown (its frame is still
  in a visible page) keeps its person from being pushed. (Its other half,
  the bell of a chat the shell frames sending its person to a tab of the
  chat's own to ask for notifications, went 2026-10-24: the shell holds a
  chat's alerts, docs/api.md, The shell.)
- **Risk:** a person with the shell open on another chat misses a
  reply's push.
- **First proof:** a person who keeps the shell open says a reply came
  without a notification.
- **Delete when:** the shell tells each frame whether it is shown (a
  message the chat's page reads, beside its theme).

## A chat's unread count counts its person's own messages

- **Observed:** 2026-10-09, unread badges (docs/api.md, `PUT
  /api/fragments/{name}/seen`). A row's `unread` counts its person's
  search entries after their mark, and an entry names no author (the
  fragment's `search_log` has no column for one, and a column added to it
  would refuse every older fragment's writes: "Objects made before a
  column", above). The shell marks the chat it shows, so a message sent
  there never counts.
- **Risk:** a message the person sends from elsewhere (the CLI's `fragment
  ask`, another device) counts as unread until they open the chat.
- **First proof:** a chat's badge after `fragment ask`, counting the
  question beside its answer.
- **Delete when:** search entries carry their record's principal (the
  fragment's log made again with the column, its lists sent again), and
  the count leaves out the person's own.

## A busy chat's push pauses itself

- **Observed:** 2026-10-03. The chat's push job runs on a channel trigger
  (`"from": "agent"`), so each agent reply is a triggered run, and an
  operation's triggers pause themselves after 120 triggered runs in an
  hour (`limits::TRIGGERED_RUNS_PER_HOUR`). Nothing unpauses a chat's.
  Since 2026-10-24 an agent's words before each tool call are a reply of
  their own (`interim`, docs/chat-records.md): the job pushes none of
  them, but each is a triggered run, so a chatty agent's turns count many.
- **Risk:** a chat whose agents reply more than about twice a minute for
  an hour stops pushing, for good, until someone runs `fragment unpause`;
  an agent that narrates every tool call of a long task gets there in a
  few turns.
- **First proof:** an `op.auto-paused` event for `notify_reply` on a
  chat.
- **Delete when:** a trigger's breaker counts runs that fail or loop
  rather than every run (or an automatic pause expires), or a chat's push
  fires once a turn, on its last reply.


## The effects sweep has no fault-injection test

- **Observed:** phase 2 slice C; reworked in the reliability pass. A
  mutation's effects apply after the facet commits it. The supervisor
  records the mutation as pending before it calls the facet; if it dies
  before applying, the next activation (or the alarm) asks the facet
  about each pending id and applies the run it recorded. The e2e proves a
  passing failure is applied by the alarm, a refusal settles for good,
  a replay applies nothing again, and a ledger row the app forges is
  never applied across a restart, but it cannot stop the node between
  the facet's commit and the supervisor's apply.
- **Risk:** a mutation whose records never appear (and no `changed`
  signal) after a crash in that window, if the sweep is wrong. The
  pending row must be stored before the facet commits: that rests on the
  runtime's output gate holding the facet call until the row is written,
  which no test shows.
- **First proof:** a crash in production between facet commit and apply.
- **Delete when:** celld (or a test build of the cell) offers a fault
  point after the facet call, and an e2e kills the node there and finds
  the records after restart.

## App channels grow without bound

- **Observed:** phase 2 slice C, as MODEL decided: app channels keep
  their records forever in the supervisor's SQLite (`events` and `ops`
  keep 90 days and 10 000 records).
- **Risk:** a busy chat grows the supervisor's database until celld's
  per-object limits or replication cost bite.
- **First proof:** a channel past ~100 000 records, or phase 3 storage
  numbers.
- **Delete when:** `fragment.json` can declare a channel's retention
  (count or age) with a platform ceiling, enforced and tested.

## A connection's consent is not bound to the browser that began it

- **Code fix (2026-10-20; rollout pending):** the legacy REST authorize
  path is removed. Fragment now completes a provider authorization-code
  flow under `auth::Bound`, the same live platform session, a one-use
  Registry transaction and S256 PKCE, before importing credentials into
  Pipes. Missing `connection_oauth` fails closed. See
  [the contract, evidence and remaining operational work](pipes-browser-binding.md).
  The local `connections` lane covers two users/browsers and replay/restart;
  vendor configuration and refresh still require controlled staging validation.
- **Observed before this fix:** the 2026-10-16 security audit's round-trip work (H3).
  Every round trip that comes back to the platform now finishes only in
  the browser that began it (`auth::trip_bound`). A connection's does not
  come back: `POST /api/connections/{provider}/authorize` asks WorkOS
  Pipes for a consent URL for the person's WorkOS user (`POST
  /data-integrations/{provider}/authorize {user_id}`, spike S5), the shell
  opens it, and WorkOS finishes the provider's OAuth itself. WorkOS's docs
  (the Pipes guide, the authorize reference, the OpenAPI spec,
  workos-node) say nothing of who may complete it; its own Pipes widget
  binds the flow to the opener's browser with a handoff token (an
  internal GraphQL mutation, `requireHandoff: true`) that the REST
  endpoint does not offer. Paul's test, 2026-10-17, settled it: in the old
  WorkOS "Fragment" project's Staging environment
  (environment_01M39VWAVVQ05KTFQ9VR22GAQV), a REST consent link for user A
  (user_01M3ZQ6R42PT5581BTPT4VHE85: an `/authorize-redirect` URL, no
  `return_to`), opened in an Incognito window signed in to nothing but a
  different Google account, showed Google's consent; Allow completed it,
  and WorkOS emitted `pipes.connected_account.connected`
  (event_01M551YAQEBPM4EBX8AJJVGJYN, 13:48:39Z), attaching the other
  person's Google to user A, with `established_by_user_id` = user A. The
  REST consent is bound to no one.
- **Remaining gate:** the user's staging Google OAuth client and WorkOS
  provider are now configured, but the provider remains Disabled. The
  new platform callback, matching client-secret binding and catalog
  configuration must be reviewed before Paul deploys. The documented
  WorkOS import/update contract supports this design; no real grant,
  import or refresh has been tested here. Previously issued unbound
  URLs/accounts must be resolved before enabling a previously used
  environment: removing our old path cannot invalidate those URLs.
- **Risk:** confirmed, not hypothetical. Whoever holds a consent link gets
  the account of whoever approves it: someone who can connect accounts
  sends another person their own link, that person approves at Google,
  and their Google (Paul's test granted gmail.modify and gmail.send,
  Drive, Calendar, Docs, Sheets and Contacts) is attached to the sender's
  WorkOS user, which the sender's agents then read and write through
  their swap. Deleting the connected account at WorkOS does not revoke it
  at Google.
- **First proof:** done, Paul's test above (2026-10-17). Any deployment
  that enables a connection provider reproduces it with one link.
- **Delete when:** the new browser-bound flow and its regression tests
  are reviewed, deployed by Paul and proven in controlled staging,
  including imported-token refresh with the matching OAuth client.
  Provider enablement and real grants need separate authorization.

## An own key's sign-in finishes at the person's one computer

- **Observed:** the audit's H3 fix (2026-10-17). An own key's callback
  asks the signed-in person's computer for the sign-in it began, and that
  computer is `default_computer_of(person)` (`computer::SignedIn::of`):
  one computer per person (decision 13), never one a URL names.
- **Shortcut:** none today; the binding rests on that one-to-one.
- **Risk:** once a person has several computers, the callback would ask
  only the first, and a sign-in begun from another would come back as
  "not under way". Naming the computer in the URL again would reopen H3.
- **First proof:** a person's second computer, connecting an own key.
- **Delete when:** a person may hold several computers: the sign-in's
  computer is kept with its state where the callback can find it from the
  bound state alone (the registry, or a cookie of the binding's), never
  from the URL, with the H3 checks run against it.

## Running runs are checked a few at a time

- **Observed:** phase 2 slice D. A run whose Workflow ended without
  reporting (an error outside a step, or a lost instance) is found by the
  poll backstop, which checks the 25 longest-running runs per pass. Runs
  asleep for days are checked every pass, and could crowd out a stuck
  one when there are more than 25.
- **Risk:** a stuck run stays `running`, holding its inbox record's
  place under the cap.
- **First proof:** a fragment with more than 25 long-sleeping jobs.
- **Delete when:** runs record when they were last checked and the pass
  takes the least recently checked, or the Workflow reports its own end
  from a `finally` step.

## A blob upload is not resumable, and whole files pass through memory

- **Observed:** phase 2 slice E. A blob upload is one request (at most
  256 MiB) streamed through the router into R2; celld cannot resume a
  multipart upload on another node or after a restart, so a node that
  restarts mid-upload fails it, and the CLI retries the whole file. The
  CLI also reads each file whole (as sync always has) and holds it while
  it uploads.
- **Risk:** large media over slow links, and memory spikes on the
  machine that syncs.
- **First proof:** a video over a few hundred MB, or a laptop syncing a
  folder of them.
- **Delete when:** uploads go in resumable parts the cell tracks (or
  straight to the bucket with a signed URL once the fleet bucket offers
  one), and the CLI streams files from disk.

## A bad upload can delete a good copy of the same blob

- **Observed:** phase 2 slice E. When uploaded bytes do not hash to the
  sha they claim, the cell deletes the key they were stored under. An
  editor uploading wrong bytes for a sha at the same moment someone
  uploads the right ones can delete the right ones.
- **Risk:** a pointer whose bytes are gone until the file is synced
  again. Only an editor of the fragment can do it.
- **First proof:** a report of a blob gone right after an upload.
- **Delete when:** uploads land under a temporary key and move to the
  content key only once verified (R2 has no rename: this needs a copy,
  or celld's native blob API).

## Blobs in git history lose their bytes after the grace period

- **Observed:** phase 2 slice E, as MODEL decided (latest versions only).
  A rollback of `live` to a commit older than the grace period (7 days)
  that named a large file serves pointers whose bytes are gone (404).
- **Risk:** surprise when rolling far back a fragment with large media.
- **First proof:** a rollback past a week in a fragment with media.
- **Delete when:** someone needs old versions of large files (the
  "Photoshop file" exception in MODEL), and a per-fragment policy keeps
  the bytes named by the last N live commits.

## Local nodes keep their computers apart through wrangler's and workerd's internals

- **Observed:** 2026-10-05, several sessions running the e2e on one Mac.
  wrangler 4.145.0, at a dev session's teardown, removes `docker ps
  --filter ancestor=<tag>` for each image tag it built; Docker resolves
  the filter to the image's ID, which the same source builds in every
  worktree, so one node's stop removed other runs' computers mid-run (a
  run's end removed another's live container, both runs on one commit).
  It removes the containers and not the `-proxy` sidecars workerd
  1.20260930 runs beside them, and miniflare stops workerd with SIGKILL,
  so workerd's own cleanup (which removes both) never runs: 109 orphaned
  sidecars were running at once. crates/devstack/src/containers.rs builds
  each project's images from Dockerfiles of its own, labeled for it (a
  distinct image ID, every layer cached), and removes what its nodes left
  by the names workerd gives them, `workerd-<worker>-<class>-<id>` and
  `…-proxy` for each object in its state's `v3/do/<worker>-<class>/`, a
  container only when it carries the project's label.
- **Risk:** it leans on three internals at the pinned versions: the
  teardown's ancestor filter, workerd's container names, and miniflare's
  state layout. A wrangler or workerd that changes the names or the layout
  leaves the containers again (the removal finds none to remove; it
  cannot reach another project's, whose computers carry its own label);
  one that matches containers some other way could reach other runs'
  computers again. A per-run image is also a new image ID per run: an
  image config each, every layer shared.
- **First proof:** `docker ps` lists `workerd-fragment-*` containers
  after every run that made them has ended, or a run's computers die
  when another worktree's node stops.
- **Delete when:** wrangler removes a dev session's containers by its own
  tag or label (not the image ID) and its sidecars with them, or lets
  workerd drain at shutdown: then `scope_images`, `remove`, and their
  calls in the e2e and `xtask dev` go. Moving the wrangler pin checks it.

## A local node's port is a front door of ours, past wrangler's proxy

- **Observed:** 2026-10-30, CI's e2e flakes. `wrangler dev` (4.145.0,
  workerd 1.20260930) answers on its port through its ProxyWorker, a
  Durable Object in a second workerd that pools its connections to the
  runtime running the Worker. That runtime closes a connection idle 5 s,
  and a request the proxy writes onto one as it closes is lost: a GET or
  HEAD it asks again, anything else it answers 500 "Network connection
  lost" (a test's PUT, run 40341674686; a CLI approval's POST,
  40404853530; triggers' inbox, 40980576877). A node started from a
  shell lost 25 to 60 of 192 PUTs sent on fresh connections 4.985 s
  after a burst. Pages' module requests also went unanswered behind it
  under load (the chat's fit, #704's and #707's shell reloads), which a
  Worker of a few lines reproduced under wrangler and never with Chrome
  sent straight to the runtime (0 of 1,882 loads). So the node's port
  is `crates/devstack/src/front.rs`: each connection a client opens
  passed byte for byte to the runtime, nothing pooled between them.
- **Shortcut:** wrangler names the runtime's address only in its debug
  output, where its proxy is told of each start and restart
  (`reloadStart`, then `reloadComplete` with the address; a restart moves
  the port). A node runs with `--log-level debug` always, front.rs reads
  its output for those lines, and a connection opened during a restart
  waits for the next address (at most 90 s), as wrangler's proxy holds a
  request. A start whose runtime is never named fails, saying so. The
  lines come from wrangler's inspector, which an inherited
  `VSCODE_INSPECTOR_OPTIONS` (VS Code's JavaScript Debug Terminal) turns
  off, so wrangler never gets that variable.
- **Risk:** the lines are wrangler's internals, at the pinned version. A
  wrangler that no longer writes them fails every node's start (loudly:
  no client falls back to the proxy's path). Debug output makes a node's
  log larger, `xtask dev`'s included.
- **First proof:** a node start failing with "did not name its runtime's
  address", `cargo test -p fragment-devstack --test front_door --
  --ignored` answering a PUT through the front door with anything but
  200, or `--test debug_terminal` (started and reloaded with that
  variable set) failing.
- **Delete when:** wrangler's proxy stops losing requests to its pool
  (the upstream issue: a pool that lets an idle connection go before
  the runtime closes it, or asks again a request no byte of which was
  answered), or the local node runs Miniflare itself with its own port,
  no proxy between: then front.rs, the debug level and the readers in
  `Node::start` go.

## The cell signs code.storage tokens for any repo a fragment names

- **Observed:** H1, and in the cell since phase 2 (`cell/src/keys.rs`).
  A code.storage JWT is signed for whatever repo the fragment's
  supervisor names: the binding between a fragment and its repo is the
  cell's own record. A repo's id is opaque (code.storage makes it), so
  the signer cannot derive it from the caller.
- **Risk:** a supervisor made to ask for another fragment's repo gets a
  token for it (at most fifteen minutes).
- **First proof:** a bug in the cell that lets a caller choose the repo
  a token names.
- **Delete when:** tokens are signed only for the repo the fragment's
  creation recorded, checked on every token, with a test.

## A socket opened with a key outlives that key's revocation

- **Observed:** phase 4 slice A. Every request resolves its key live, so
  a revoked key is refused from its next request. Since the speed pass
  (#13), a signed-in `__live` socket asks the registry again at its
  first frame after `LIVE_IDENTITY_MS` (60 s) and is closed with 4001
  when its credential no longer names its principal. But the check runs
  only when the client sends a frame: a `__live` socket that only
  listens, and every `__watch` socket, stays open until it reconnects or
  the membership it rests on changes (member and agent removals do
  close sockets).
- **Risk:** a stolen, revoked key keeps a change feed it already had:
  reads only, and only of fragments that identity can still read.
- **First proof:** a revoked key's feed still receiving frames.
- **Delete when:** revoking a key tells the fragments in its identity's
  index to close the sockets tagged with that key (the index cell
  already lists them), with an e2e check; or every socket re-resolves
  its key on a timer, not only at a frame it sends.

## The registry is one Durable Object for the fleet

- **Observed:** #156 (problem 8). Every request whose answer depends on
  who asks resolves its key or session live in the one `Registry` cell
  (cell/src/registry.rs, `NAME`): one round trip each, never more, and
  none for a page or file anyone who may see the fragment gets alike
  (the signin and identities lanes count them). A CLI or agent call, a
  model call, an operation, an app route, a socket's connect: 1 each (a
  `__live` socket 1 more a minute while it sends frames); a members-only
  fragment's page: 1 a file; the shell's load (cell/shell/shell.js
  `start`, `load`): 4, then up to 3 a chat (its members, its channels,
  its newest message) and 1 an app (its card), then the open chat's
  frame (a mint, a redemption, its files and its socket).
- **Risk:** throughput, and where it lives. Cloudflare puts one object at
  500–1,000 simple requests a second: a person with 50 chats and 10 apps
  asks it about 170 times a shell load, so a few such loads a second
  fill it. A person far from it pays a round trip there for each signed
  request, and while it restarts (a deploy) every signed request waits.
- **First proof:** the `Registry` namespace's requests a second
  (Workers analytics) sustained past 200, or its wall time's p99 past
  20 ms.
- **Delete when:** BANKS answers instead (docs/finite-integration.md,
  FIN-11); or, if the load comes first: the shell reads its sidebar in
  one request (its list's rows carry what a row shows, or one call
  answers every chat under one resolved identity), which takes the 3 a
  chat away; then, if still near the line, sessions and keys move to
  objects named by what they resolve (a session by its token's hash, a
  key by itself), written by the registry as they change, which keeps
  only what must be unique (emails, sign-in subjects, identities).
  Each lookup stays one live round trip: no cache outlives a revocation
  (rule 7).

## Video steps are off

- **Observed:** 2026-10-03, when OpenRouter was cut (Paul: "I thought we
  didn't need openrouter now that we're using ai gateway?"). Images moved
  onto Workers AI (FLUX.1 [schnell] through the AI binding,
  cell/src/ai.rs); Workers AI's catalog has no video model (2026-10-03), so
  `job.ai.video` is a step the platform refuses ("video steps are off
  until they run on Cloudflare": `fragment_core::media::Refusal::VideoOff`),
  and OpenRouter's key, its fake and `Usage::Billed` went with it.
- **Risk:** a fragment that made videos (meatproxy's `fragment:ai`,
  docs/published-fragments.md) cannot, and an agent asked for one can
  only say so.
- **First proof:** a held run whose error is that refusal.
- **Delete when:** a video model runs on Cloudflare (Workers AI's
  catalog, or a provider through AI Gateway's Unified Billing), metered
  from its usage as images are, with `job.ai.video` back as steps and an
  e2e check that a video is drawn, stored as a blob and charged; or Paul
  drops videos, and `job.ai.video` goes with this entry.

## An icon's drawings skip the AI Gateway

- **Observed:** 2026-10-24, building icons (docs/api.md, Icons). FLUX.2
  [klein] 4B takes its input as a multipart form, and the AI binding
  refuses a gateway for a form's streamed body ("AI Gateway does not
  support ReadableStreams yet"); sent as bytes, the gateway answers 400.
  So `js::ai_run_form` calls the binding with no gateway: a drawing has no
  gateway log, no `cf-aig-log-id`, and no gateway metadata (its payer's
  opaque id), and is charged from the binding's own `cf-ai-neurons`.
- **Risk:** a drawing's cost cannot be matched to a gateway log, as a
  job's image is; a drawing's failures are seen only in the cell's log
  (`icon.step-failed`).
- **First proof:** an `icon:` meter row whose drawing has no log in the
  gateway's.
- **Delete when:** the gateway takes a form's body through the binding;
  then `ai_run_form` passes the gateway as `ai_run` does.

## The price book is the core's defaults

- **Observed:** phase 3. Every ledger charges with
  `PriceBook::defaults()` and the configuration's operator keys' prices
  (cell/src/config.rs `configured_book`): the list prices of 2026-10-02
  and a 50% margin. Decision 4 puts pricing in the deployment's
  configuration.
- **Risk:** a self-deployer cannot set their own margin or prices without
  a fork; a Cloudflare price change needs a code change and a deploy.
- **First proof:** a deployment that wants another margin.
- **Delete when:** `xtask deploy` renders a price book from the
  deployment's config (a versioned file) into a Worker variable that
  `configured_book` reads (each ledger prices with its release's book
  since decision 62, so a changed one reaches them all with its deploy),
  with a test of a book's change.

## A plan has two writers: the registry, and an operator's `plan` command

- **Observed:** orgs and seats (docs/billing.md). The registry pushes a
  seat holder's plan to their ledger (registry/orgs.rs), and an operator
  may still set a plan straight on the ledger (`POST
  /api/ledger/{person}/plan`), which the e2e uses to make a guest. The
  registry's next push for a seat holder wins. A seat's state has one
  writer, the registry: the ledger's `seat` command went on 2026-10-16
  (its `seq` could run ahead of the registry's, and every later push was
  then answered as done and dropped), and a ledger it left ahead refuses
  the registry's push as stale, naming its order, which the registry's
  next passes.
- **Risk:** an operator's override of a seat holder's plan is silently
  undone at their seat's next change.
- **First proof:** an operator who set a plan by hand and finds it reverted.
- **Delete when:** the ledger's monthly allowance (approved 2026-10-07)
  replaces plans' rules: the `plan` command goes, operators comp or end
  seats instead, and the e2e makes guests through seats.

## A fragment's git repository is not metered, and storage is sampled daily

- **Observed:** phase 3 (cell/src/meter.rs). A fragment's storage meter
  samples its own SQLite and its blobs once a day from its alarm, as
  byte-hours since the last sample (its app's database: the entry "An
  app's database answers to the app's own code alone"). Its
  code.storage repository is not sampled (code.storage publishes no size
  or price to us). A computer's saves are the next entry's.
- **Risk:** git storage is free to its owner; a fragment that grows and
  shrinks within a day is billed at the day's sample.
- **First proof:** a repository at code.storage larger than a fragment's
  SQLite.
- **Delete when:** code.storage reports a repository's size (or we host
  git), sampled as `Storage { class: git }`, and samples run hourly from
  a cron the plan named (docs/ledger.md).

## A computer's saves are not metered

- **Observed:** 2026-10-16, writing the platform explainer. A computer
  keeps its newest three saves of `/data` in R2
  (`computers/<id>/backups/`, `fragment_core::computer::SAVES_KEPT`;
  docs/computers.md, "Saves and what a wake restores"). The Computer DO meters only awake time and operator keys
  (`Usage::Awake`, `Usage::Key` in cell/src/computer.rs); no meter
  samples its saves, so docs/ledger.md had said they bill its owner when
  nothing did.
- **Risk:** a computer's storage is free to its owner and paid by the
  platform: three saves of a `/data` that grows large cost R2's rate
  three times over, with no cap or notice.
- **First proof:** a computer whose saves in R2 cost more a month than
  its owner's credit covers.
- **Delete when:** the Computer DO samples its saves' bytes (the
  records it keeps, `DirectoryBackup`'s archives) as
  `Storage { class: r2 }` to its owner on a schedule, as the Fragment DO
  samples its blobs, with a test that a save's bytes reach the ledger.

## A run in flight and the platform's own writes go on past the overdraft

- **Observed:** phase 3. Past its owner's overdraft a fragment refuses
  the writes a principal asks for (mutations and jobs, posts, file
  writes, deploys, storage tokens, blobs, its inbox, replays: `writable`
  in cell/src/meter.rs), and since 2026-10-03 its cron, channel and file
  triggers start no runs (each recorded `blocked` with the ledger's
  reason: `start_run` in cell/src/jobs.rs, the ledger section's e2e). But
  a run already in flight still writes (its mutations, publishes, file
  writes) and starts the jobs it calls, and the platform's own records
  (an agent's `joined` on its `tasks`: cell/src/runs_on.rs) still append.
- **Risk:** a run that started before the overdraft writes a read-only
  fragment for as long as it runs (its AI steps are refused by the
  ledger all the same); the platform's own records are a few bytes each.
- **First proof:** a read-only fragment whose events show a run's
  writes landing after its owner went past the overdraft.
- **Delete when:** each step of a run asks the owner's standing before
  it writes (a step past the overdraft fails for good, saying why, so
  the run is held and replays after a top-up), with an e2e check; the
  platform's own records stay (they cost nothing, and an agent must hear
  where it joined).


## A postable channel's retention is fixed

- **Observed:** a channel people may post to keeps its newest
  `limits::POSTED_KEPT` (10 000) records (cell/src/channels.rs `append`),
  whatever the fragment would choose; fragment.json cannot declare
  another number (it would need a column in the installed code's channel
  table, and its parsing).
- **Risk:** a busy chat loses its oldest messages past 10 000, and a
  small one could not ask for less.
- **First proof:** a person asks where the start of a long chat went.
- **Delete when:** a channel's `keep` is declarable (bounded, with this
  as its default) and tested past it.

## Files depend on code.storage, a hosted service with no self-hosted twin

- **Observed:** 2026-09-29, while scoping self-hosting (the sandbox
  investigation, docs/sandbox.md at the tag `celld-final`). Every
  fragment's files and history live in a code.storage repo
  (`cell/src/cs.rs`, `cli/src/sync.rs`), reached only through its REST
  API: repos, repo urls, branches, file metadata and
  reads, commits, commit packs with expected-parent CAS, merges,
  and restore commits, under ES256 JWTs minted with
  the org key. No git smart-HTTP is used. The only other implementation
  is the test fake (`crates/fakes/src/codestorage.rs`), which keeps its
  state in memory or one JSON file.
- **Risk:** fragment is not self-hostable while one of its planes is a
  vendor's hosted service; an outage, a price change, or a deprecation
  at code.storage stops every fragment's files at once, and a
  self-deployer (docs/cloudflare-v1.md, decision 4) must hold a
  code.storage org.
- **First proof:** the first deployment that cannot or will not use
  code.storage (a self-hosted or air-gapped fleet), or a code.storage
  outage on fragment.club.
- **Delete when:** a self-hostable service answers the same REST subset
  on durable storage (git on disk or in the bucket), with the fake's
  rules as its conformance suite, and the e2e passes against it as well
  as against the fake.

## Channel drafts' refusals have no e2e

- **Observed:** the cut (2026-10-02) took the lane that drove
  `PUT /api/f/<name>/channels/<channel>/draft` (cell/src/channels.rs
  `draft_api`: a record its poster is writing, sent to the channel's
  readers as `draft` frames, never stored). Phase 4's computers and
  real-Hermes lanes drive it again through the bridge, and a page's
  socket hears the frames; nothing drafts as a stranger or past the pace.
- **Risk:** a change lets a stranger draft on a channel, or drops the
  pace, and nothing says so.
- **First proof:** a draft from someone who may not post.
- **Delete when:** a lane checks that a stranger's draft is refused and
  that a draft past the pace is refused (429).

## The hosted lane runs part of the suite, and not from CI

- **Observed:** phase 2 (2026-10-03): the e2e runs on local workerd,
  whose limits differ from Cloudflare's: it enforces no CPU, memory,
  subrequest or concurrency limit (spike S1), and its Workflows keep a
  sleep as a timer in the process, so one never wakes after a crash of
  `wrangler dev`. Those checks are `skip`s, counted in every run. Phase 7
  (2026-10-03, `claude/phase-7`) built the hosted lane (`cargo xtask e2e
  --hosted`, crates/e2e/src/hosted.rs): it runs on a preview the sections
  whose declared needs a preview meets. The rest are skips there, each
  saying why: those that script a vendor fake (the model: ai, ledger,
  chat; code.storage's git: create, files,
  ops, effects, site, sync, blobs, appfiles; a local upstream or push
  service: jobs, triggers, push, channels' posts), the node's (restart,
  lockdown, isolation, share, export: it crashes the node mid-build and
  scripts the stub's runtime), and the whole deployment's
  (identities, signin, ledger's operator). Nothing runs it from CI.
- **Risk:** a limit, a Workflow resume, or a skipped section's behaviour
  on real vendors breaks unseen.
- **First proof:** any skip in a run's summary, local or hosted.
- **Delete when:** the hosted lane runs from CI against a branch
  deployment, each section that needs a fake today reaching a target on
  the deployment itself instead (a fragment's own route as a job's
  upstream, its inbox as a webhook's, the real model within the run's
  paid calls), and the local skips made there.

## An app's database answers to the app's own code alone

- **Observed:** phase 2; 2026-10-06 (issue #156, problem 4). Every bound
  on an app facet's SQLite runs in the app's own realm (cell/platform.mjs),
  which its code can patch: a mutation that leaves the database over
  16 MiB rolls back (507), but a query, a route (`fetch`), or code that
  patches the platform's own writes past it (celld's hard stop 4 MiB
  above went with the fork). Nothing outside the realm can stop it:
  workerd allows a facet no `query_only`, `max_page_count` or authorizer
  (its pragma allowlist), and a query or a route is async, so no
  transaction spans one to roll back. Nor can the supervisor read the
  facet's size: only the realm can say it. So the storage meter does not
  count the app's database (until 2026-10-06 it asked `__size` daily,
  which loaded the app's code, a billed dynamic worker, every day, to
  meter at most 16 MiB of an honest app, under a cent a month).
- **Risk:** the platform pays for app SQLite its owner is not billed for:
  at most 16 MiB a fragment for an honest app ($0.0032 a month at list),
  without bound for an app that writes outside its mutations.
- **First proof:** a Fragment namespace whose stored bytes (Cloudflare's
  usage) are well over the sum of its fragments' sampled `sqlite`.
- **Delete when:** the runtime lets the supervisor read a facet's
  database size or bound it (a facet storage limit), and the meter
  samples it, with a test of a query that writes past the cap.

## An export reads an app's database through the app's own realm

- **Observed:** 2026-10-13 (docs/api.md, Export). The supervisor cannot
  read a facet's SQLite (the entry above), so `GET /api/f/{name}/database`
  asks platform code in the facet (`__dump`, cell/platform.mjs), which
  runs in the author's realm: an app's code can patch what it answers.
  The supervisor holds its answer to its shape and its page's bounds, not
  to the truth. An integer past 2^53 crosses as a JavaScript number,
  rounded; a table's `rowid`, unless a column of its own, is not dumped.
- **Risk:** an app that patches the platform's code exports what it
  likes of its own database, to its own owner (the only one who reads
  it); an import of a dumped table renumbers its implicit rowids.
- **First proof:** a dump's rows that disagree with what the app's own
  queries answer.
- **Delete when:** the runtime lets the supervisor read a facet's
  database itself (a facet SQL handle for its parent, or a backup of its
  file), and the export reads that.

## A chat's transcript is the platform's reading of the chat template's records

- **Observed:** 2026-10-13 (docs/api.md, Export). An export renders a
  chat fragment's `transcript.md` from its `chat` and `work` records
  (`fragment_core::export::transcript_part`), which are the chat
  template's and its bridges' (docs/chat-records.md): platform code that
  knows one template's records, which the rule (docs/cloudflare-v1.md)
  calls a design bug. It reads the shape generically where it can (a
  message is the search's convention: `text`, `attachments`) and passes
  over any kind it does not know; the records themselves are exported
  whole beside it (`channels/*.jsonl`).
- **Risk:** a change to the records' shape (a new kind, a renamed field)
  leaves transcripts missing it until the platform's renderer follows.
- **First proof:** a chat whose transcript lacks something its page shows.
- **Delete when:** the chat template renders its own transcript (a query
  of its own the export asks for its fragment, paged), or Paul decides the
  platform may read the chat records' shape (then this moves to the
  special-case inventory).

## An export can fail when its computer saves three times as it is copied

- **Observed:** 2026-10-13 (docs/api.md, Export). An export copies its
  computer's newest good save a piece at a time through the Computer DO,
  which keeps three saves and deletes each pushed out at once. Three new
  saves while one save is copied (a busy computer saves at each work's
  end, 30 s settled, and every 15 minutes) delete the one being copied:
  the export fails `source_changed`, saying to export again. Nothing holds
  the save for it.
- **Risk:** a busy person's export of a large `/data` fails, and they
  press again.
- **First proof:** an export failed `source_changed` on a deployment.
- **Delete when:** the Computer DO keeps a save an export copies until it
  is copied (a hold its forget passes over), with a test that saves three
  times mid-copy.

## A blob larger than the zone's request limit cannot be uploaded

- **Observed:** phase 2. A blob route takes up to 256 MiB, but
  Cloudflare refuses a request body past the zone plan's limit (100 MB
  on Free and Pro) before the Worker sees it.
- **Risk:** `fragment sync` of a file between 100 MB and 256 MiB fails.
- **First proof:** a 101 MB file synced to a deployment on a Free zone.
- **Delete when:** blobs upload in parts (R2 multipart through the
  cell), each under the limit, with a test of a 150 MB file.

## The dev proxy may answer an early refusal with its own 500

- **Observed:** phase 2. Under `wrangler dev`, a Worker that answers
  before a chunked upload ends (the body limit's 413) sometimes reaches
  the client as miniflare's own 500, "Network connection lost". The e2e
  accepts that answer locally, saying so; Cloudflare's edge has no such
  proxy.
- **Risk:** none in production; a weaker local check.
- **First proof:** already present.
- **Delete when:** the hosted lane checks the 413 itself, or miniflare
  passes the early answer through.

## The Hermes image patches Hermes' own boot

- **Observed:** phase 4 (`images/hermes/Dockerfile`). The preloaded
  gateway is hermes-boot's main program (spike S3b), so the image removes
  upstream's `/etc/cont-init.d/02-reconcile-profiles`, whose
  `hermes_cli.container_boot` would start an s6-supervised gateway beside
  it from a restored `gateway_state.json`. The build fails if that script
  is not there to remove, or another cont-init script names
  `container_boot`.
- **Risk:** a Hermes release that starts a gateway at boot some other
  way: two gateways serve one home.
- **First proof:** the first Hermes upgrade after v0.21.5.
- **Delete when:** upstream lets a container whose main program is the
  gateway skip its reconciler (or ships a preloadable, unsupervised
  gateway main program: S3b's "Upstream Hermes" list), proven by the
  real-Hermes lane on an image without the removal.

## A Hermes turn's end is read from its reactions

- **Observed:** phase 4, against the real image. Relay has no
  turn-level end; the bridge reads its processing hooks' reactions
  (`👀` off, then `✅`/`❌`). Hermes brackets a message it took while its
  gateway was starting twice, the first empty (docs/hermes-relay.md), so
  the relay runtime ends a turn at `❌`, at `✅` only once it said
  something (Hermes ends no person's turn without a word), and, stopped,
  at `👀` off. No clock (#156 cut the 1.5 s and 20 s windows).
- **Risk:** a Hermes release that ends a person's turn saying nothing,
  or cancels one the bridge did not stop, leaves it running until the
  idle bound (15 minutes, then an error); one that stops reacting does
  so for every turn (loudly: the Docker rung's turns never end). Hermes'
  clarify questions are read by their glyphs (`❓`, `✏️`) the same way,
  and a clarify card's "Other" by its positional id (`other`): one Hermes
  renames leaves "Other" a button again, answered as before (asked for
  in words after), and `docker.rs`'s `other_answered_in_words_on_its_card`
  fails.
- **First proof:** a turn that ends "the agent stopped answering" though
  Hermes answered,
  or a reply posted under a `said` turn rather than its message's.
- **Delete when:** Hermes' Relay sends a turn's end and its questions as
  structure, proven by the real-Hermes lane.

## A routine's silence is read from Hermes' English warning

- **Observed:** production, 2026-10-26 (Skyler's agent: a routine every
  15 minutes posted Hermes' "⚠️ The model returned only a silence marker
  for a message that needed a reply" at each fire). Relay carries no
  `reply_expected` (v0.21.7, and upstream's main that day), so Hermes
  answers a routine's bare `[SILENT]` with that warning; the relay runtime
  drops it for a routine's turn alone, matched whole in its English words
  (`wire::UNEXPECTED_SILENCE`; docs/bridge.md, "A routine with nothing to
  tell says nothing").
- **Risk:** a Hermes release that rewords it, or a profile whose
  `display.language` is not English (the image sets none; an agent may
  set its own), posts the warning at every fire again. A model that
  brackets the marker with words (`[SILENT] nothing changed`), which
  Hermes' cron's looser rule would deliver nothing for, posts them.
- **First proof:** a routine's chat showing "The model returned only a
  silence marker"; on a rewording, the Docker lane's
  `a_routine_may_say_nothing` fails first.
- **Delete when:** Relay carries `reply_expected` (or a turn's end as
  structure, the entry above): the bridge then marks a routine's inbound
  as expecting no reply, and Hermes' own silence ends a turn the bridge
  can read; proven by the real-Hermes lane.

## The screen writes Hermes' lease file, not through Hermes

- **Observed:** 2026-10-07 (`images/bridge/src/lease.rs`,
  `images/hermes/boot/src/desktop.rs`). Take over is each agent's Hermes
  Bot Desktop lease, so its `computer_use` refuses while a person holds
  the screen; but Hermes changes its lease only in its own process
  (`tools/bot_desktop/lease.py`) or through its TUI gateway's
  `display.lease.*` RPCs, which this image does not run. The bridge
  writes the file as Hermes' `_transition` does (its `lease.lock` flock,
  `lease.json.tmp` renamed over it, the epoch one on): Hermes' file
  format, not an interface it promises. Likewise the idle stop reads the
  desktop's `activity` and `env` files' times as Hermes' `stop_if_idle`
  does (that watcher too is its TUI gateway's), and the screen touches
  `activity` as Hermes' own viewer does.
- **Risk:** a Hermes release that changes the lease's file, path or
  lock: the screen and the agent's tools then disagree on who holds it
  (a person typing while the agent acts, or an agent refused for good);
  or that renames the activity file: desktops stop under their viewers,
  or never.
- **First proof:** the Docker rung's `two_agents_two_desktops` on a new
  Hermes: it reads the lease with Hermes' own code, has the agent's
  `computer_use` refuse while a person holds the screen, and has an
  unwatched desktop stop and a watched one stay.
- **Delete when:** Hermes offers a lease and an idle stop outside its TUI
  gateway (a gateway control verb, or its RPCs on the messaging gateway),
  and the image uses them, proven by the same test.

## Each agent's Hermes profile config is rewritten at every boot

- **Observed:** phase 4 (`hermes-boot`). A profile's `config.yaml` is
  its model block (tier, the model intercept, `x-fragment-agent`), its
  name (`agent.system_prompt`: docs/computers.md, "Its name, in our
  Hermes image"), its skills' directories, its vision model, its browser and its terminal,
  written whole at each boot; anything Hermes or the agent wrote there
  is lost. Hermes' managed scope (`/etc/hermes/config.yaml`, which the
  boot writes for what every profile shares) cannot carry the rest: it
  is one layer for every profile, its `${VAR}`s expand against the
  gateway's own environment, never a profile's `.env`, and Hermes v0.21.5
  reads some keys (`browser.*`) with `read_raw_config`, past it. A merge
  into the profile's file would need a YAML parser in the boot, or
  Hermes' own Python per profile per boot.
- **Risk:** an agent's own `hermes config set` lasts until the computer
  sleeps.
- **First proof:** an agent that changes its own Hermes settings.
- **Delete when:** an agent's settings live in its fragment (its
  `agent.json`, phase 5's agent template) and the boot merges them into
  the profile's config rather than replacing it, proven by a setting
  that survives a wake.

## The swap's HTTPS is proven only under wrangler dev

- **Observed:** phase 4 (`cell/src/computer.rs` `egress_swap`). The
  computers lane drives the swap from the stub's scripted agent over
  plain HTTP; the real-Hermes lane sends a stock `curl https://…` from
  Hermes' terminal, with the placeholder from its environment, through
  `interceptOutboundHttps` with Cloudflare's local CA. Both send the
  swapped request to `FRAGMENT_SWAP_UPSTREAM`, not a real provider, and
  neither runs on Containers. The four operator keys and the Google
  connection have never reached their real vendors through it.
- **Risk:** Containers' CA, or a real provider's TLS and auth, differ
  from the local proxy's, so a connection works in dev and not hosted.
- **First proof:** the first hosted computer using a connection.
- **Delete when:** the hosted lane makes one swapped call to a real
  provider from a computer on a preview deployment.

## An operator key is metered per call, not per what its vendor counts

- **Observed:** Paul, 2026-10-04 (`fragment_core::price::DEFAULT_KEYS`).
  The swap holds and settles one `key` unit a call the provider
  answered. Perplexity's Sonar Pro briefs also bill tokens and a request
  fee ($0.02 to $0.04 a brief at list), priced as a $0.005 search; xAI's X Search bills posts and
  profiles fetched, priced as a 20-post call ($0.12); ElevenLabs bills
  minutes of music (and characters of speech), priced as a minute ($0.15).
  Since 2026-10-09, Firecrawl is priced as one 10-result search (2 Hobby
  PAYG credits, $0.01), also charged for a basic one-credit scrape. FAL
  is priced as a 1 MP FLUX.2 Klein 9B image ($0.006) on every authenticated
  request, including queue polls and result reads. Browser Use is priced
  as five minutes at $0.02/hour ($0.02/12) on every control request,
  including stop, excluding its $5/GB proxy traffic. Sources:
  [Firecrawl](https://www.firecrawl.dev/pricing),
  [FAL](https://fal.ai/models/fal-ai/flux-2/klein/9b),
  [Browser Use](https://browser-use.com/pricing), read 2026-10-09.
  X API's separate `x` key is estimated as the `x-api-finite` helper's
  default ten-post recent search ($0.05), from its
  [per-resource prices](https://docs.x.com/x-api/getting-started/pricing):
  $0.005 per post, $0.010 per user. Expanded users, batches and searches
  returning up to 100 posts can cost more; a single lookup costs less.
  Vendor daily deduplication is not reflected in our per-request meter.
- **Risk:** a brief or a long composition costs the operator more than
  it charges; a short jingle or a speech call charges the person more than
  it cost.
  Polling a FAL image charges more than the generation's list price;
  longer browser sessions and proxy traffic can cost more than the
  control calls charge. The estimates are not usage-based vendor billing.
- **First proof:** a month's vendor invoice against the ledger's `key:`
  settlements for that key.
- **Delete when:** the swap reads each answer's usage (a header or the
  body, as the model route reads tokens) and settles the vendor's own
  units against a hold of its worst case, each catalog row naming how,
  proven by a lane whose upstream fake answers usage and is charged by it.

## The Hermes image adapts Bot Desktop's browser ownership

- **Observed:** 2026-10-28, Hermes v0.21.7's desktop base includes regular
  Chromium 154, but our wrapper selected PM's Chrome for Testing 145.
  Both `agent-browser` 0.26.0 and Hermes' dock launch with debugging port
  `0`, enabling `navigator.webdriver`; switching to a nonzero port leaves
  no `DevToolsActivePort` file for their discovery. The native daemon also
  retains its first CDP endpoint across a desktop restart unless explicitly
  reconnected. A global `browser.cdp_url` loses this version's local-session
  lease fence. These were reproduced on the real image.
- **Shortcut:** the image selects its existing `/usr/bin/chromium` and
  extends pinned Hermes' local start/attach, dock and stop paths, using
  `images/hermes/desktop_browser.py` and exact source replacements guarded
  by `boot/src/desktop.rs`. The Python adapter reuses upstream's profile,
  desktop environment and process-stop helpers. It serializes starts,
  validates the live profile owner against CDP's browser PID, and gives
  each visible browser a nonzero loopback port. Tool sessions use the
  supported `agent-browser connect` command when that port changes, and
  retain their local-session fence. No new daemon, cloud browser or
  fingerprint script is introduced.
- **Risk:** private upstream boundaries can change; exact-match guards
  fail the image build if they do. A port allocation can race another
  local process; startup then fails rather than attaching to it. The
  adapter refuses profile downgrades, but rolling back to an image from
  before this guard also requires restoring its pre-upgrade save: its
  browser was 145, and the new profile may be 154's.
- **First proof:** the Docker lane's
  `a_regular_browser_is_shared_by_the_desktop_and_tools` (native tools,
  webdriver, both launch orders, cookies, tool cleanup, desktop and
  computer restarts, browser crash and stale port metadata), and
  `two_agents_two_desktops` (distinct browser PIDs/ports, human takeover
  refusing browser tools, idle stop sparing the other agent's browser).
- **Delete when:** a pinned upstream desktop supports regular Chromium
  with a nonzero CDP port, starts and reattaches it through its existing
  local-session lease fence, and owns its shutdown independently of tool
  session cleanup. Remove the adapter and all replacements together once
  these same Docker checks pass against that upstream implementation.

## Hermes' direct FAL client reads process credentials, not profile scope

- **Observed:** 2026-10-09, Hermes v0.21.6's
  `tools/image_generation_tool.py::_submit_fal_request` calls the module's
  global `fal_client.submit`. Its pinned SDK 0.13.1 caches `FAL_KEY` from
  `os.environ`, while the multiplex gateway keeps credentials in
  `agent.secret_scope`. It fails with no process key and reuses a process
  key across profiles when one exists.
- **Stand-in:** the image's `tool_keys.py`, installed by its preload,
  binds a fresh explicit-key SDK client to each direct image request and
  its result handle. The key is the profile's placeholder, no environment
  mutation, and the client closes when its result is read. Other image
  selections use upstream's path. The Docker test drives two scopes
  interleaved against a fake HTTP transport, then reads their results
  outside either scope and checks each retained its own placeholder.
- **Risk:** the adapter calls a pinned upstream private function and SDK
  client property; an upstream change could bypass it or break cleanup.
- **First proof:** the Docker profile test against the next pinned image.
- **Delete when:** Hermes' direct native image path uses an explicit SDK
  client from its active profile scope, including the result worker.

## Hermes' tool workers can take an ended thread's interrupt

- **Observed:** #776, 2026-10-30: the real-Hermes Docker lane's
  `fragment list` came back "[Command interrupted]" (exit 130) in 0.08 s.
  Hermes v0.21.7 keys a tool's interrupt by thread id. When a turn starts
  while a background memory review runs, it cancels the review by its
  thread's id, and the review may have just ended. A new thread usually has
  the last ended one's id, and `_registered_tool_worker` clears a worker's
  bit only as it ends. So the next tool worker reads that bit as its own.
- **Stand-in:** the image's `tool_threads.py`, installed by its preload,
  clears a tool worker's bit as it starts, before it is registered for
  interrupts, so no real interrupt can have set it yet.
- **Risk:** the adapter replaces a pinned upstream private function that
  `tool_executor` and `terminal_approval_batch` look up at each call; an
  upstream change could bypass it. The adapter doesn't cover other threads
  that read their own bit (the execution thread already clears its bit as a
  turn starts), or an interrupt that lands after the next worker started.
- **First proof:** the Docker test `hermes_tool_workers_start_clean` runs
  pinned Hermes' worker on recycled ids, with and without the adapter.
- **Delete when:** upstream's `_registered_tool_worker` starts its worker
  clean: that test's first assertion fails, saying so.

## Hermes keeps its providers' variable names from its terminal

- **Observed:** Paul, 2026-10-04 (`images/hermes/boot`: `hermes.rs`
  `credentials_sh`). Hermes v0.21.5 never passes a name of its own
  providers' keys to its terminal (`_HERMES_PROVIDER_ENV_BLOCKLIST`:
  `PERPLEXITY_API_KEY`, `XAI_API_KEY`, `ELEVENLABS_API_KEY` among them),
  whatever `terminal.env_passthrough` lists. The image also writes them to
  the profile's `credentials.sh`, which its terminal sources as a session's
  shell starts (`terminal.shell_init_files`), so they are in its snapshot.
- **Risk:** a Hermes release that scrubs those names from the snapshot, or
  ignores `shell_init_files`, takes the operator keys from the agents'
  terminals (their skills say the key is not offered); and one sourced at
  a session's start keeps a value removed meanwhile until the session
  ends (a placeholder then refused at the swap, never a key).
- **First proof:** the hermes lane's check that a stock curl in Hermes'
  terminal finds `$PERPLEXITY_API_KEY`, failing on a Hermes upgrade.
- **Delete when:** Hermes lets a profile pass a name it keeps (an
  allowlist of its own), or the catalog's names for those providers are
  ones Hermes does not keep, proven by the same check with no
  `credentials.sh`.

## An agent's execute_code makes its temp files in Hermes' home

- **Observed:** 2026-10-07, the Docker lane's
  `a_tools_temp_files_are_its_work` (`images/hermes`; docs/computers.md,
  "Our images"). An agent's terminal commands, foreground and background,
  make their temp files in its work (`/data/work/<profile>/tmp`, through
  its profile's linked `cache/scratch`), but an execute_code script makes
  its own in the gateway's scratch, `/data/hermes/cache/scratch`. Hermes
  (v0.21.5, and v0.21.6 alike) builds a script's environment with a
  scrub that keeps `TMPDIR`,
  `TMP` and `TEMP` and drops every `HERMES_*` name it does not list, its
  `HERMES_SCRATCH_DIR` marker among them (`tools/code_execution_env.py`,
  `_scrub_child_env`). So its `apply_scratch_tmp_env`, run after, takes the
  gateway's `TMPDIR` for one a user set, and keeps it. No profile setting
  reaches it: Hermes resolves `TMPDIR` process-wide.
- **Risk:** what a script leaves in its temp directory is saved with
  Hermes' home, not its agent's work, and every agent's scripts share that
  directory (Hermes prunes what is idle 24 hours). When the work moves to a
  sandbox of its own (step 4 of docs/durable-computers.md), those files
  stay behind.
- **First proof:** before step 4, one agent's script reading a file
  another's left in that directory; after it, a file a script left there
  that its agent's terminal, in the sandbox, cannot find.
- **Delete when:** our terminal backend runs execute_code's scripts (step
  3: Hermes runs them through any backend but `local`), or Hermes keeps
  its marker for them. Either fails the lane's check that pins the
  script's temp file in the gateway's scratch, which then holds it to the
  work with the rest.

## An agent's Bot Chat is found by its chat's label

- **Observed:** 2026-10-08, Bot Mode in our Hermes image
  (docs/computers.md, "Bot Mode in our Hermes image"). Which chat is an
  agent's own, and so its Hermes Bot Chat, the image finds by label: in
  the agent's list for its owner, the chat its owner owns labelled
  `<label>-chat` beside the agent's `<label>`, as the shell's `makeAgent`
  makes it (`bots::bot_chat`; `fragment ask`'s `direct_label` finds the
  same). A chat's name has a suffix of its own, so it is looked up at a
  boot, as agents come, and every few seconds while one is missing.
  Nothing on the platform says which chat is an agent's.
- **Risk:** an agent made another way (the CLI, the API) has no Bot Chat
  of ours, and Hermes makes one of its own that the person never sees; a
  person with two chats of that label (one made by hand) may get either.
- **First proof:** a bot whose roster and `message_agent` never appear in
  its own chat (no `botmode.titled` event for it, or no
  `botmode.chat_found`).
- **Delete when:** the agent fragment names its own chat (its
  `agent.json`, written by whatever makes the two), and the image, the
  shell and the CLI read that.

## Bot Mode's roster lists the gateway as a teammate

- **Observed:** 2026-10-08, Bot Mode in our Hermes image. Hermes lists its
  default profile in every bot's roster as `@hermes`, always
  (`tools/bot_mode_probe.py`, `_roster`); in our image that profile is the
  gateway's own, which runs no agent's turns. Its `profile.yaml` says so
  ("this computer's gateway, not an agent: never message it").
- **Risk:** a model messages `@hermes` anyway: Hermes runs a turn as the
  default profile, whose model calls name no agent, so the model route
  refuses them and the sender is told the delivery failed, having spent
  a turn on it.
- **First proof:** a `message_agent` call whose target is `hermes` in a
  hosted run's model calls.
- **Delete when:** Hermes lets an install leave its default profile out
  of the roster, or our gateway's own profile is an agent's.

## A bot's answer to a teammate outlives the teammate's wait only while awake

- **Observed:** 2026-10-08, Bot Mode in our Hermes image (the keeper,
  `images/hermes/botmode.py`). A bot's `message_agent` waits for its
  teammate's answer in a background process of Hermes' (up to 30
  minutes), and the keeper delivers the message by a `fragment ask --wait`
  of its own. Neither is saved: a sleep, a crash or a restart while the
  teammate's turn runs ends both. Hermes keeps the delivery's claim for
  good (its receipts never expire), and nothing settles it.
- **Risk:** the sender never hears the answer, and never tells its owner;
  the answer is in the teammate's chat, and the sender's owner sees it
  there only if they look.
- **First proof:** a hosted run where a computer sleeps while one bot's
  teammate is still answering it, and the sender's chat says nothing more.
- **Delete when:** a delivery's wait is the journal's: the keeper, at its
  start, finds each claim of its own left unsettled and settles it from
  the teammate's chat (the hand-off's record is `dm-<delivery>`), and the
  sender's wake comes from that receipt rather than from a process.

## Own keys are kept by the person's computer

- **Observed:** Paul, 2026-10-04 (`computer.rs` `own_keys`). A person's
  own key for an `own` provider is sealed in their computer's Durable
  Object, since a person has one computer (decision 13) and the swap
  resolves there.
- **Risk:** a person with a second computer gives their key again; one
  whose computer is deleted loses it.
- **First proof:** a second computer per person.
- **Delete when:** own keys live in the person's own cell (docs/secrets.md)
  and every computer of theirs asks it, proven by two computers swapping
  one key.

## A connection's state comes from Pipes' connected-account API, untried on staging

- **Observed:** Paul, 2026-10-04 (`keys.rs` `pipes_state`). A guest is
  given a connection once `GET
  /user_management/users/{user}/connected_accounts/{provider}` says
  `connected` (workos.com/docs/reference/pipes/connected-account), so no
  token is minted to tell. Spike S5 proved the token and authorize calls
  on staging, not this one; the fake answers as the reference does.
- **Risk:** staging answers it otherwise (another path, a 404 for every
  account), and no agent is ever given Google.
- **First proof:** the hosted lane with a connected Google account.
- **Delete when:** the hosted lane reads a connected account's state on
  staging, proven by its guest given the connection.

## The swap reads a request's body whole

- **Observed:** phase 4. `egress_swap` reads the guest's body (at most
  32 MiB) before it sends it on, as `egress_api` does, rather than
  streaming it.
- **Risk:** an upload to a connection (Drive, a large attachment) over
  32 MiB is refused, and a large one holds the isolate's memory while it
  goes.
- **First proof:** an agent uploading a large file through a
  connection.
- **Delete when:** the swap streams the body (its length passed on),
  proven by an upload larger than the cap.

## An agent's sync and deploy reach code.storage directly

- **Observed:** decision 17's branch (`claude/skills`). The fragment CLI
  in our Hermes image acts as its agent through the API egress, with no
  key (cli/GUIDE.md, "As an agent"), but `fragment sync`, `deploy`,
  `rollback` and `drafts` still go to code.storage itself, with the
  15-minute, repo-scoped token the platform mints for the agent.
- **Risk:** that token is a credential in the guest, which otherwise
  holds none (decision 43), for as long as the command runs. Locally,
  where a container under `wrangler dev` cannot reach the dev stack's
  code.storage fake at the machine's loopback, the egress sends the
  guest's calls on to it, its token with them (docs/computers.md, The
  fragment API; the computers lane's "through its egress" checks): the
  path, not yet the token swap.
- **First proof:** phase 6's exit, an agent building and deploying an
  app from a chat, on a preview.
- **Delete when:** a computer's sync and deploy go through its API egress
  (the files and deploy routes, or a code.storage intercept that swaps
  the token in), proven by the hermes lane deploying an app from a chat.

## `fragment rollback` right after a two-step deploy picks its first step

- **Observed:** 2026-10-03, deploys after a rollback. code.storage merges
  three ways, so a deploy whose `live` has diverged from `main` (a
  rollback's restore commit, then main moved) first restores live to
  their merge base, then merges main in, which then takes main's files
  whole (`fragment_core::codestorage::Promotion`). Every deploy is the
  cell's (`go_live`, `POST /api/f/{name}/deploy`; `fragment deploy` asks
  it), which holds its plane lock across both moves, so a refresh or poll
  between them waits and pins the second: the first is never served.
- **Risk:** `fragment rollback` without `--to` right after such a deploy
  restores the first move's files (the deploy the earlier rollback
  undid), not the files the rollback served (by first parents, as the
  fake lists; the service's date order picks among main's commits, the
  contract's question 4).
- **First proof:** `fragment drafts` after a deploy that followed a
  rollback listing its "(first, live back to …)" commit as the one before
  live, and a bare `fragment rollback` landing on it.
- **Delete when:** `rollback`'s default target is the commit live served
  before its tip (skipping a deploy's first step), or code.storage merges
  with the source's tree.

## An open card keeps its computer awake, and any other restart under it still loses its card

- **Observed:** 2026-10-05, Paul on p5 missed an approval card's hour and
  his agent stopped answering: the idle sleep under the card cut its
  turn, and Hermes folded every later message into the cut request,
  asking its approval again. The bridge now holds the keepalive while a
  turn waits on its card (images/bridge/src/engine.rs, `finish`;
  docs/bridge.md, "A card keeps its computer awake"), the P6 stopgap
  Paul agreed (docs/durable-computers.md). The fold itself is closed by
  P5 (2026-10-06: the boot closes a cut turn in Hermes' session, and the
  next turn is told what was cut; `a_turn_cut_by_a_restart_is_closed_and_told`
  in images/bridge/tests/docker.rs).
- **Risk:** an unanswered card costs its life awake (an hour by default;
  a runtime may ask up to `PROMPT_TTL_MS_MAX`, a day). A restart for any
  other reason while a card is open (an owner's sleep, a crash, a deploy,
  an image pinned) still cuts the turn and closes its card `expired`
  before its `expiresAt`: the owner's answer has no turn to go to (docs/durable-computers.md).
- **First proof:** a computer's awake time dominated by turns waiting on
  cards; or an owner who answers a card after a restart and sees nothing
  happen.
- **Delete when:** P6 lands: a card outlives its turn (a late answer
  starts a new turn, told what was cut), so the keepalive can be let go
  while a card waits again, proven on the real-Hermes lane by a sleep
  under a card whose answer, after the wake, is acted on once.

## Preview cards drive Browser Rendering over a CDP client of our own

- **Observed:** 2026-10-06 (issue #156, head scratcher 4). A card's shot
  (cell/src/card.rs, `take`) acquires a Browser Rendering session, drives
  it over a CDP client of ours, and closes it. The same binding now has
  `quickAction("screenshot", {url, viewport, screenshotOptions,
  gotoOptions, waitForTimeout})`: one call, the image back, its browser
  time in `X-Browser-Ms-Used`, billed by the browser hour alone (no
  concurrent browsers). It is not used because `wrangler dev`'s local
  binding (wrangler 4.145.0, miniflare 5.20260930.0-alpha) answers only
  the session routes (Cloudflare: "not yet supported in local development
  mode"; it throws "The RPC receiver does not implement the method
  'quickAction'"), so dev and the e2e would lose their local shot; and it
  needs a compatibility date of 2026-03-24 or later, where the cell's is
  2026-01-01 (moving it brings ~25 flags, `delete_all_deletes_alarm` and
  `websocket_close_reason_byte_limit` among them).
  Re-checked 2026-10-19 (the review of code we wrote where something off
  the shelf exists), and kept: quick actions still cannot take this shot.
  Cloudflare's docs (the product is now Browser Run) still have no local
  `quickAction` (only `wrangler dev --remote`, which the e2e's node is
  not), and its screenshot answers with the image alone: no console
  messages, exceptions or log entries, which the page's report (status's
  `page`, `page.errors`; cec664ad, a day after this entry) is made of
  (`card::Heard` hears `Runtime.exceptionThrown`,
  `Runtime.consoleAPICalled` and `Log.entryAdded`). `@cloudflare/puppeteer`
  in the shim, on the same session routes, was weighed and not taken: it
  swaps about 200 lines of plumbing here for a library the Worker bundles
  and every isolate evaluates as it starts, for a shot once a deploy; the
  shim would make the shot's decisions (its waits, its qualities), which
  the shim leaves to Rust; and the session, its keep-alive billing and
  the events `Heard` reads would all stay.
- **Risk:** the CDP client is ours to keep speaking Browser Rendering's
  session routes as `@cloudflare/puppeteer` does; a change there fails
  every try (`card.failed`) while the pages serve on. The deployment pays
  the concurrent-browser charge quick actions do not have (docs/ledger.md,
  Assumptions).
- **First proof:** `card.failed` events on a preview whose message is a
  CDP or session error.
- **Delete when:** a quick action reports what the page said as it
  loaded (its exceptions, console errors and log entries), or the page's
  report comes from somewhere else; `wrangler dev` answers `quickAction`
  locally (or a fake at the vendor boundary stands in for it there,
  `FRAGMENT_BROWSER_URL` as `FRAGMENT_AI_URL` does for the model route);
  and the cell's compatibility date reaches 2026-03-24 (a change in the
  runtime's behaviour its pull request names): `take` becomes one
  `quickAction("screenshot", …)` billed from `X-Browser-Ms-Used`, and the
  session's acquire and release, `Cdp`, `billed_ms`'s keep-alive and the
  concurrent-browser assumption go.

## A fragment's sign-in can be forced to someone else's session

- **Observed:** the fragment.boats design (2026-09-25; its "Not in
  scope"). `__signin?token=` sets a fragment's site cookie from any
  redemption its link carries, and checks nothing the browser began
  first.
- **Risk:** someone mints a redemption for themselves on X and sends a
  victim to `X/__signin?token=…` within its 60 s. The victim is then
  signed in on X as them, and what they type there lands in the
  attacker's account (login CSRF). It reaches no one's own data.
- **First proof:** a report of someone finding themselves signed in on a
  fragment as another person.
- **Delete when:** `__signin` sets a nonce cookie before the platform hop
  and the redemption must carry it, with an e2e that redeems another
  browser's token and is refused.

## An app's own answers may set cookies named as the platform's

- **Observed:** the fragment.boats design (its "Not in scope"). An app's
  `fetch` answer reaches the browser with its own `Set-Cookie` headers
  (`cell/src/js.rs`, `Facet::fetch`), platform names included
  (`__Host-fragment_site`, `fragment_anon`, `fragview`).
- **Risk:** only to its own fragment. It can sign its visitors out
  there, or give two anonymous visitors one `anon:` principal. It cannot
  mint a session the registry knows.
- **First proof:** an app that sets or clears a platform cookie.
- **Delete when:** the router strips the platform's cookie names from an
  app's answer, with an e2e app that tries.

## Neither domain sends HSTS

- **Observed:** the fragment.boats design (its "Not in scope"). No answer
  carries `Strict-Transport-Security`, and neither zone is preloaded.
- **Risk:** a first visit over http on a hostile network can be held to
  http. Session cookies are `Secure` and `__Host-`, so none is sent there.
- **First proof:** any report of a downgrade.
- **Delete when:** both zones send HSTS (at cutover, when they move to
  Cloudflare), checked by the hosted lane.

## A chat's page not framed by its owner learns a rename at the agent's next turn

- **Observed:** 2026-10-14, building one name and picture for an agent:
  a chat's page reads `__people` once per identity, and no live event
  tells a page of one fragment that another (the agent's) changed its face.
- **Shortcut:** framed by its owner's shell, the page takes the roster's
  title and picture, which follow the owner's list live; anywhere else
  (a guest's page, a chat in a tab of its own) it asks `__people` again as
  each of an agent's turns ends (templates/chat/site/chat.js, `again`).
- **Risk:** someone else's open chat shows an agent's old name and picture
  until that agent's next turn ends, or a reload, when the agent was
  renamed from Settings rather than in a turn.
- **First proof:** a second person's browser on a shared chat, the owner
  renaming the agent in Settings: their page keeps the old name until the
  agent speaks.
- **Delete when:** a fragment's pages hear its agents' face changes (an
  agent fragment's install telling the chats it is in, as its joining
  does).

## A wake from a snapshot waits for its guest to be heard (#362)

- **Observed:** 2026-10-16, canary rounds r4 and r6 on the preview: a
  container Cloudflare restored from a snapshot took the DO's execs for
  about a second and then answered nothing; its guest never egressed,
  and its start had looked fine, so nothing fell back (#362).
- **Shortcut:** a start from a snapshot is up only once its guest reads
  its own view (`GET /api/computer`) within `ALIVE_MS`, 45 s; unheard,
  it is answered as a snapshot that would not start (docs/computers.md,
  Lifecycle). An image that never reads its view wakes 45 s late, from
  the image and its save.
- **Risk:** a dead one costs its person 45 s and a restore; a guest that
  reads its view later than the wait (a backoff past it) is judged dead
  and woken again from its save; and a container that dies after its
  guest's first read is still not caught here (its sleep's bounds end it).
- **First proof:** the logs' `{snapshot: "dead"}` lines against the
  snapshot wakes (`restored {from: "snapshot"}`).
- **Delete when:** Cloudflare fixes #362 and a canary round shows no
  `snapshot: "dead"` across its snapshot wakes; or snapshots go
  (`FRAGMENT_COMPUTER_SNAPSHOTS=off`), when the wait goes with them.

## SimpleX retains the computer's root trust and rewindable state

- **Observed:** the SimpleX review (2026-10-22 to 2026-10-28) left these
  obligations. The current protocol is in docs/bridge.md, "SimpleX";
  decision 32 in docs/cloudflare-v1.md records Paul's product choices.
- **Risk:** passwordless sudo lets a prompt-injected agent read the
  root-only door key or drive the unauthenticated loopback bot API.
  The signed link ticket and separate door key do not fence root.
- **Delete when:** the agent has no sudo, or the bridge and cores run
  outside the agent's root authority, with forged-owner tests.
- **Risk:** a restore before linking loses the contact until relinked;
  saving immediately narrows that window but cannot eliminate it.
  A newer core may migrate its databases so an older image cannot open
  them. Check upgrade and rollback compatibility in each core image round.
- **Delete when:** messenger state no longer rewinds (durable-computers.md,
  option F), and core upgrade/rollback is proved against real databases.
- **Risk:** one core per linked agent has no overall resource cap;
  voice attachments have no transcript text for search; the original
  review did not prove the shell link pane or "via SimpleX" in a browser.
- **Delete when:** core resource limits, searchable voice text without
  double transcription, and browser coverage of linking and attribution
  close those gaps. Existing tests and contracts remain in place.
