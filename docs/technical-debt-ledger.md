# Technical debt ledger

Shortcuts are allowed only through this ledger (engineering style §1).
Each entry says what the shortcut is, where and why, its risk, and the
condition that deletes it. An entry without a delete condition is
unfinished design, not debt.

## Every call still carries its release's deploy time

- **Shortcut:** an object of a release before #826 read
  `x-fragment-release-deployed` to restart onto a newer caller's
  release, so `routed::stamp` still sets it on every call into an object
  (`fragment_core::release::DEPLOYED`, never renamed), though no release
  reads it now.
- **Risk:** none beyond a header's bytes on each internal call.
- **Delete when:** no release from before #826 can be live (it has
  reached production, and a rollback past it is no longer allowed); then
  drop it from `stamp`, the constant, and MODEL.md's truth-map row.

## Hermes' explicit auxiliary vision overrides native image routing

- **Shortcut:** Hermes' explicit auxiliary backend turns images into text
  even for models that read them, so the `fragment-native-images` plugin
  routes images past it, four resized ones of 256 KiB a call.
- **Risk:** a change to the pinned internals it rides bypasses its bounds.
- **Delete when:** upstream supports an auxiliary fallback without
  forcing text, and one per-call image-count/encoded-byte budget for
  attachments and screenshots passes
  `native_images_are_bounded_and_unknown_models_keep_the_describer`
  (images/bridge/tests/docker.rs) under the route's 6 MiB cap.

## The Hermes image adapts model preflight deployment waits

- **Shortcut:** a release guard's refusal lasts minutes, past Hermes'
  stale limits, so the `fragment-deploy-wait` plugin (`deploy_wait.py`)
  wraps Hermes' `_StreamingCall._open_chat_stream` and watchdog hooks and
  a not-dispatched preflight refusal waits up to six minutes.
- **Risk:** an upstream change could dispatch twice or hide a stall.
- **Delete when:** Hermes natively supports a bounded preflight wait
  with these timer, retry, cancellation and no-dispatch guarantees, and
  `hermes_waits_out_model_preflight_deploying` and
  `hermes_deployment_adapter_contract` pass without the adapter.

## Hermes external skills need an image copy-on-write plugin

- **Shortcut:** Hermes' reviews write external skills in place, so the
  `fragment-skill-fork` plugin copies ours into the profile first.
- **Risk:** it leans on `_find_skill`, `_resolve_supporting_file` and
  the review's read marks.
- **Delete when:** Hermes natively supports managed-aware copy-on-write
  with the same ownership and fresh-read guarantees, and the skill-fork
  check in `the_hermes_image` (`skill_fork.py`) passes without it.

## The Hermes image patches Hermes' own boot

- **Shortcut:** hermes-boot runs the gateway itself, so the image removes
  upstream's `/etc/cont-init.d/02-reconcile-profiles`
  (images/hermes/Dockerfile), failing the build if another script names
  `container_boot`.
- **Risk:** a Hermes that starts a gateway some other way runs two.
- **Delete when:** upstream lets a container whose main program is the
  gateway skip its reconciler (or ships a preloadable, unsupervised
  gateway main program), proven by the real-Hermes lane without it.

## Hermes' Relay carries no structure for a turn's end or a routine's silence

- **Shortcut:** Relay has no turn-level end and no `reply_expected`, so
  the relay runtime (images/bridge/src/runtime/relay) reads a turn's end
  and its questions from reactions and glyphs, and drops a routine's
  English silence warning (`wire::UNEXPECTED_SILENCE`).
- **Risk:** a rewording leaves turns running or posts the warning at
  every fire (`other_answered_in_words_on_its_card` and
  `a_routine_may_say_nothing` fail first).
- **Delete when:** Relay carries a turn's end, its questions and a
  routine's `reply_expected` as structure, proven by the real-Hermes lane.

## The screen writes Hermes' lease file, not through Hermes

- **Shortcut:** Hermes changes its Bot Desktop lease only in its own
  process or its TUI gateway, which the image does not run, so the
  bridge writes the lease file (images/bridge/src/lease.rs) and the idle
  stop reads its activity files (images/hermes/boot/src/desktop.rs): a
  format, not an interface.
- **Risk:** a changed lease lets a person and the agent share a screen.
- **Delete when:** Hermes offers a lease and an idle stop outside its
  TUI gateway, and the image uses them, proven by
  `two_agents_two_desktops`.

## The Hermes image adapts Bot Desktop's browser ownership

- **Shortcut:** upstream's dock and tools launch Chromium on CDP port 0,
  which sets `navigator.webdriver`, and cannot find one on another port,
  so images/hermes/desktop_browser.py and source replacements guarded by
  images/hermes/boot/src/desktop.rs run the base's Chromium on a nonzero
  port through Hermes' start, dock and stop paths.
- **Risk:** private upstream code changes (the build fails); rolling
  back past it needs the pre-upgrade save.
- **Delete when:** a pinned upstream desktop supports regular Chromium
  with a nonzero CDP port, starts and reattaches it through its own
  lease fence, and owns its shutdown apart from tool cleanup; then the
  adapter and replacements go together once
  `a_regular_browser_is_shared_by_the_desktop_and_tools` and
  `two_agents_two_desktops` pass against it.

## Each agent's Hermes profile config is rewritten at every boot

- **Shortcut:** Hermes' managed scope cannot carry per-profile settings,
  so hermes-boot writes each `config.yaml` whole
  (images/hermes/boot/src/hermes.rs `profile_config`).
- **Risk:** an agent's own `hermes config set` lasts until it sleeps.
- **Delete when:** an agent's settings live in its `agent.json` and the
  boot merges them into the profile's config, proven by a setting that
  survives a wake.

## Hermes' direct FAL client reads process credentials, not profile scope

- **Shortcut:** Hermes' `_submit_fal_request` uses the SDK's global
  client, keyed from the process, so the image's `tool_keys.py` gives it
  an explicit-key client per request
  (`tool_profiles_select_offered_operator_keys`).
- **Risk:** a change to that private function bypasses it.
- **Delete when:** Hermes' direct native image path uses an explicit SDK
  client from its active profile scope, including the result worker.

## Hermes' tool workers can take an ended thread's interrupt

- **Shortcut:** Hermes keys a tool's interrupt by thread id and clears a
  worker's bit only as it ends, and a new thread reuses an ended one's
  id, so a cancelled review's bit can interrupt the next tool. The
  image's `tool_threads.py` clears a tool worker's bit as it starts.
- **Risk:** it replaces a pinned private function
  (`_registered_tool_worker`); an upstream change could bypass it.
- **Delete when:** upstream starts its worker clean:
  `hermes_tool_workers_start_clean`'s first assertion then fails, saying
  so.

## Hermes keeps its providers' variable names from its terminal

- **Shortcut:** Hermes withholds its providers' key names
  (`PERPLEXITY_API_KEY`…) from its terminal, so the image writes them to
  `credentials.sh` (images/hermes/boot/src/hermes.rs `credentials_sh`).
- **Risk:** a Hermes that scrubs them takes operator keys from agents.
- **Delete when:** Hermes lets a profile pass a name it keeps, or the
  catalog's names for those providers are ones it does not keep, proven
  by the hermes lane's stock curl finding `$PERPLEXITY_API_KEY` with no
  `credentials.sh`.

## An agent's execute_code makes its temp files in Hermes' home

- **Shortcut:** Hermes' execute_code drops `HERMES_SCRATCH_DIR`, so its
  temp files land in `/data/hermes/cache/scratch`, shared by every agent
  (`a_tools_temp_files_are_its_work` pins it).
- **Risk:** work moved to a sandbox of its own leaves them behind.
- **Delete when:** our terminal backend runs execute_code's scripts, or
  Hermes keeps its marker for them: `a_tools_temp_files_are_its_work`
  then fails, and holds the file to the work with the rest.

## An agent's own chat is found by its chat's label

- **Shortcut:** nothing on the platform says which chat is an agent's, so
  the bridge takes its owner's `<label>-chat` (images/bridge/src/api.rs
  `own_chat`; the CLI's `direct_label` finds the same).
- **Risk:** an agent made another way has no home chat until one is made
  with that label; a second chat of that label may be taken instead.
- **Delete when:** the agent fragment names its own chat (its
  `agent.json`), and the bridge, the shell and the CLI read that.

## A Hermes release has no rollback

- **Shortcut:** a computer keeps `fragment_core::computer::SAVES_KEPT`
  saves, so soon none predates an image change: Hermes moves forward
  only.
- **Risk:** a bad Hermes stays until a fix; pinned back, it opens
  databases a newer one migrated.
- **Delete when:** a computer keeps the save its first start on a new
  image restored, apart from its three, until its next image change,
  and a pin back restores it.

## The agent's page derives the model an agent runs; the image does not say it

- **Shortcut:** nothing the image chooses reaches the platform, so the
  model rule is in hermes-boot (`MainModel::of`, `Tier::of`) and again in
  templates/agent/site/agent.js (`runs`).
- **Risk:** they drift, and the page names the wrong model.
- **Delete when:** the image says what each agent runs (`PUT
  /api/computer/agents/{a}/model` or the like), its owner's view
  carries it, and the page shows that.

## The swap's HTTPS is proven only under wrangler dev

- **Shortcut:** the lanes drive the swap (cell/src/computer.rs
  `egress_swap`) only through local interception to a fake upstream.
- **Risk:** a connection works in dev and not hosted.
- **Delete when:** the hosted lane makes one swapped call to a real
  provider from a computer on a preview deployment.

## The swap reads a request's body whole

- **Shortcut:** `egress_swap` reads up to `EGRESS_BODY_MAX_BYTES`
  before sending on.
- **Risk:** larger uploads are refused; large ones hold memory.
- **Delete when:** the swap streams the body (its length passed on),
  proven by an upload larger than the cap.

## An operator key is metered per call, not per what its vendor counts

- **Shortcut:** the swap charges one `key` unit a call at an estimate
  (`fragment_core::price::DEFAULT_KEYS`, sources beside it).
- **Risk:** calls charged above or below what they cost.
- **Delete when:** the swap reads each answer's usage and settles the
  vendor's own units against a hold of its worst case, each catalog row
  naming how, proven by a lane whose upstream fake answers usage.

## Own keys assume one computer per person

- **Shortcut:** a person has one computer (decision 13), so own keys are
  sealed in it (cell/src/computer.rs, `own_keys`), and an own key's
  sign-in callback finds it by `default_computer_of`, never the URL (the
  audit's H3).
- **Risk:** a second computer needs the keys again; a deleted one loses
  them.
- **Delete when:** own keys live in the person's own cell, proven by two
  computers swapping one key, and a sign-in's computer is found from its
  bound state alone, never the URL, with the H3 checks run against it.

## Connections are untried against real WorkOS Pipes

- **Shortcut:** connection sign-ins (cell/src/connections.rs) and their
  state (cell/src/keys.rs `pipes_state`) have answered only the fake.
- **Risk:** Pipes answers otherwise, or imported tokens never refresh;
  consent links the old REST path issued stay valid.
- **Delete when:** the browser-bound flow is reviewed, deployed by Paul
  and proven in controlled staging (docs/pipes-browser-binding.md),
  imported-token refresh and a connected account's state included.
  Enabling providers needs its own authorization.

## An agent's sync and deploy reach code.storage directly

- **Shortcut:** a computer's `fragment sync`, `deploy`, `rollback` and
  `drafts` hold a 15-minute repo token
  (`fragment_core::codestorage::guest_api`).
- **Risk:** a credential in a guest that otherwise holds none (decision
  43).
- **Delete when:** a computer's sync and deploy go through its API
  egress (the files and deploy routes, or an intercept that swaps the
  token in), proven by the hermes lane deploying an app from a chat.

## A wake from a snapshot waits for its guest to be heard (#362)

- **Shortcut:** a container restored from a snapshot may answer nothing
  (#362), so a snapshot start counts only once its guest reads its view
  within `fragment_core::computer::ALIVE_MS`, else it wakes from its
  save.
- **Risk:** a dead restore costs the wait and a restore.
- **Delete when:** Cloudflare fixes #362 and a preview round shows no
  `snapshot: "dead"`, or snapshots go (`FRAGMENT_COMPUTER_SNAPSHOTS=off`).

## A computer's saves are not metered

- **Shortcut:** the Computer DO meters awake time and keys, not its
  `SAVES_KEPT` saves in R2.
- **Risk:** the platform pays for a large `/data`, three times over.
- **Delete when:** the Computer DO samples its saves' bytes as
  `Storage { class: r2 }` to its owner on a schedule, as fragments do
  their blobs, with a test that a save's bytes reach the ledger.

## An export can fail when its computer saves three times as it is copied

- **Shortcut:** nothing holds the save an export copies; three newer
  saves delete it (`source_changed`, cell/src/export.rs).
- **Risk:** a busy person's export of a large `/data` fails.
- **Delete when:** the Computer DO holds a save until its export has
  copied it, with a test that saves three times mid-copy.

## SimpleX retains the computer's root trust and rewindable state

- **Shortcut:** the SimpleX door and cores (images/bridge/src/simplex)
  run beside an agent with passwordless sudo, their state in
  `/data/simplex`, rewound with the computer's saves.
- **Risk:** an injected agent reads the door key or drives the loopback
  bot API. **Delete when:** the agent has no sudo, or the bridge and
  cores run outside its root authority, with forged-owner tests.
- **Risk:** a restore to before a link loses the contact; a newer
  core's databases may not open in an older image. **Delete when:**
  messenger state no longer rewinds, and core upgrade and rollback are
  proved on real databases.
- **Risk:** no overall cap on cores, no voice text for search, no
  browser test of linking. **Delete when:** core resource limits,
  searchable voice text without double transcription, and browser
  coverage of linking and attribution close those gaps.

## OpenRouter's calls are charged at their ceiling, not their cost

- **Shortcut:** an OpenRouter slot is priced at its `max_price` plus fee
  (`fragment_core::models::Upstream::ceiling`), so no call is charged
  below what it could cost; `usage.cost` is unread.
- **Risk:** people are overcharged, cached input most.
- **Delete when:** a call is charged from OpenRouter's reported cost,
  the fee and margin on it.

## A ledger's head carries `holds` and a `book` for a rollback

- **Shortcut:** heads are written for releases before decision 62
  (`fragment_core::ledger` `OldHolds`, `Ledger::written`,
  `retire_holds`).
- **Risk:** none; dead weight that reads as if it meant something.
- **Delete when:** no release before decision 62 can be deployed again
  (Paul agreeing) and no head holds any: drop `OldHolds`,
  `retire_holds`, `written`'s book, and the
  `a_head_written_here_reads_in_the_release_before` and
  `a_rollback_finds_no_hold_and_no_held_entry` tests.

## A paid call's charge is best effort

- **Shortcut:** billing never interrupts a call (decision 62), so model
  calls are charged after answering (cell/src/meter.rs `charge`): no
  usage charges nothing (`ledger.unknown-usage`), a failed send is lost
  (`ledger.charge-lost`), the gate is a minute stale.
- **Risk:** calls given away: cents, against an agent never stopped.
- **Delete when:** a call with no usage is charged from what it streamed,
  and a failed send is kept and sent again.

## The price book is the core's defaults

- **Shortcut:** `configured_book` (cell/src/config.rs) starts from
  `PriceBook::defaults()`: Workers AI's prices and the margin are code.
- **Risk:** a self-deployer (decision 4) cannot change them unforked.
- **Delete when:** `xtask deploy` renders a price book from the
  deployment's config into a Worker variable `configured_book` reads,
  with a test of a book's change.

## A plan has two writers: the registry, and an operator's `plan` command

- **Shortcut:** besides the registry's push (cell/src/registry/orgs.rs),
  an operator may set a plan (`POST /api/ledger/{person}/plan`), as the
  e2e does to make a guest.
- **Risk:** an override is undone at the seat's next change.
- **Delete when:** the ledger's monthly allowance replaces plans' rules:
  the `plan` command goes, operators comp or end seats instead, and the
  e2e makes guests through seats.

## A paid org pack is still credited, though none is sold

- **Shortcut:** orgs pay out of band now (decision 66), but `pack_paid`
  (cell/src/billing.rs) still grants a Checkout whose `fragment_payer`
  names an org, so one opened on preview before #736 is not lost.
- **Risk:** none: nothing opens such a Checkout.
- **Delete when:** three days (Stripe's retries) have passed since #736
  reached preview (2026-10-30): then the org arm of `pack_paid` goes.

## A fragment's git repository is not metered, and storage is sampled daily

- **Shortcut:** `sample_storage` (cell/src/meter.rs) samples SQLite and
  blobs daily, not the code.storage repo (it publishes no size to us).
- **Risk:** git is free; a day's peak between samples is unbilled.
- **Delete when:** a repository's size is sampled as
  `Storage { class: git }` (code.storage reports it, or we host git),
  and samples run hourly.

## A run in flight and the platform's own writes go on past the overdraft

- **Shortcut:** past the overdraft (cell/src/meter.rs `writable`) a
  run in flight still writes, and so does `append_own`
  (cell/src/channels.rs).
- **Risk:** a run writes a read-only fragment for as long as it runs.
- **Delete when:** each step of a run asks the owner's standing before
  it writes (refused for good, so it replays after a top-up), with an
  e2e check; the platform's own records stay.

## Video steps are off

- **Shortcut:** `job.ai.video` is refused
  (`fragment_core::media::Refusal::VideoOff`): Workers AI has no video.
- **Risk:** a fragment that made videos (meatproxy) cannot.
- **Delete when:** a video model runs on Cloudflare, metered from its
  usage, with an e2e check that a video is drawn, stored and charged; or
  Paul drops videos, and `job.ai.video` goes with this entry.

## An icon's drawings skip the AI Gateway

- **Shortcut:** `js::ai_run_form` calls the AI binding without the
  gateway, which refuses FLUX.2's multipart body.
- **Risk:** a drawing has no gateway log.
- **Delete when:** the gateway takes a form's body through the binding;
  then `ai_run_form` passes the gateway as `ai_run` does.

## Dev's cell is not the release's build

- **Shortcut:** the release's build takes about two minutes an edit, so
  `cargo xtask dev` builds the `devstack` profile without wasm-opt
  (xtask/src/build.rs `Profile::Dev`).
- **Risk:** what only the release's codegen shows surfaces in CI.
- **Delete when:** the release's build rebuilds a change to one of its
  files in seconds, and dev builds the release's again.

## The cell's Wasm budget was raised for launch

- **Shortcut:** `CELL_WASM_MAX_BYTES` (xtask/src/build.rs) went from
  6.0 MB to 6.5 MB so launch week's features could land.
- **Risk:** slower starts, less memory, an easier next raise.
- **Delete when:** the cell is back under 6.0 MB (first candidate: the
  export's archive builder in a Worker of its own) and the budget is
  set back to 6.0 MB.

## Local nodes keep their computers apart through wrangler's and workerd's internals

- **Shortcut:** wrangler's teardown removes containers by image ID (other
  worktrees' too) and leaves workerd's sidecars, so crates/devstack/src/containers.rs labels
  each project's images (`scope_images`) and removes its leftovers by
  workerd's names and miniflare's layout (`remove`).
- **Risk:** a wrangler or workerd that changes them leaks containers.
- **Delete when:** wrangler removes a session's containers by its own
  label and its sidecars with them, or lets workerd drain at shutdown;
  then `scope_images` and `remove` go. Moving `WRANGLER_VERSION` checks
  it.

## A local node's port is a front door of ours, past wrangler's proxy

- **Shortcut:** wrangler's proxy pools connections the runtime closes
  when idle, losing a request written as one closes, so a node's port is
  crates/devstack/src/front.rs, each connection passed to the runtime.
  It learns the runtime's address from wrangler's debug output
  (`reloadComplete`), so a node always logs at debug and never inherits
  `VSCODE_INSPECTOR_OPTIONS`, which silences those lines.
- **Risk:** the lines are wrangler's internals; a wrangler without them
  fails every node's start, loudly.
- **Delete when:** wrangler's proxy stops losing requests to its pool, or
  the node runs Miniflare itself with no proxy between: then front.rs,
  the debug level and `Node::start`'s readers go. The devstack's
  `front_door` and `debug_terminal` tests check a wrangler move.

## The dev proxy may answer an early refusal with its own 500

- **Shortcut:** the e2e accepts miniflare's 500 for an early 413
  locally, saying so (crates/e2e/src/lanes/control.rs).
- **Risk:** a weaker local check; none in production.
- **Delete when:** the hosted lane checks the 413 itself, or miniflare
  passes the early answer through.

## The hosted lane runs part of the suite, and not from CI

- **Shortcut:** local workerd has no CPU, memory or subrequest limits
  and loses a Workflow's sleep in a crash, so those checks skip; the
  hosted lane (crates/e2e/src/hosted.rs) runs by hand.
- **Risk:** a limit, a resume or a vendor breaks unseen.
- **Delete when:** the hosted lane runs from CI against a branch
  deployment, sections that need a fake reaching targets on the
  deployment itself, and the local skips made there.

## A direct chat's label is its agent's, which a branch may lack room for

- **Shortcut:** a direct chat is `<agent's label>-chat`
  (cli/src/ask.rs `direct_label`), so the shell and the CLI find the
  same one; it may exceed a branch's `limits::LABEL_ROOM_MIN_BYTES`.
- **Risk:** on a branch, such an agent is refused its direct chat.
- **Delete when:** an agent's label is held, where it is made, to the
  room every deployment leaves less `-chat`, or a direct chat is found
  by its members.

## A wipe finds a person's fragments through their lists

- **Shortcut:** a wipe finds what a person owns through their and their
  agents' lists (cell/src/ended.rs); no index names them all.
- **Risk:** a fragment its owner's list never took outlives its owner.
- **Delete when:** the registry (or a per-person index) records each
  fragment as it is made, and the wipe reads it beside the lists.

## An agent made later joins best effort

- **Shortcut:** `members::agent_made` (cell/src/members.rs) asks each
  fragment once, with no outbox (decision 64).
- **Risk:** a fragment that missed it leaves the agent off its rosters.
- **Delete when:** an outbox carries it (the person's list, retrying on
  its alarm), or a fragment reconciles a person's agents as it is read.

## Invites from before their yes are met by a sign-in

- **Shortcut:** invites mailed before decision 63 (role NULL) are met
  at sign-in (cell/src/registry/invites.rs `meet_invites`).
- **Risk:** such an invitee joins without being asked.
- **Delete when:** `INVITE_TTL_S` has passed since the release that
  carries decision 63; then `meet_invites`, the kept claims, the alarm's
  claims and the `role IS NULL` clauses go, and `invite_claims` with
  them (a step that drops it).

## The registry is one Durable Object for the fleet

- **Shortcut:** every signed request resolves its key or session live in
  the one `Registry` (cell/src/registry.rs), so a revocation holds at
  once.
- **Risk:** throughput (500 to 1,000 requests a second) and distance.
- **Delete when:** its load comes (past 200 a second, or p99 past
  20 ms): the shell's sidebar in one request; then sessions and keys in
  objects of their own, each lookup still one live round trip, so no
  cache outlives a revocation.

## Objects made before versioned schemas

- **Shortcut:** `fragment_core::migrate::adopt` takes an object from
  before 2026-10-19 whose tables are step 1's (`Class::variants`); any
  other refuses every call (`schema_refused`).
- **Risk:** such objects stay refused; a Registry refuses everything.
- **Delete when:** the deployments made before 2026-10-19 (the e2e
  preview, p5, preview.finite.build) are reset or retired, or those
  objects deleted (Paul's call).

## An app facet's operation ledger has no schema version

- **Shortcut:** an app facet's database is its author's, so
  `_fragment_ops` is `CREATE TABLE IF NOT EXISTS` in each app facet
  (cell/platform.mjs), outside the steps and the schema lock.
- **Risk:** a new column breaks every older facet's mutations.
- **Delete when:** the facet's ledger takes versioned steps of its own,
  or its shape is held by a check as the platform objects' are.

## The browser half of web push is not driven by a test

- **Shortcut:** the lanes subscribe through `__push-sub` or a fake
  `PushManager`; `__sw.js` and `alerts-sw.js` meet no real push service.
- **Risk:** subscribing or a notification breaks unnoticed.
- **Delete when:** a manual check on a hosted fleet is recorded, or a
  browser test can run with a local push service.

## A chat the shell frames is "looked at" while the shell is

- **Shortcut:** a chat page's `looking` is its document's visibility
  (templates/chat/site/chat.js), true in a hidden shell pane.
- **Risk:** a reply to a hidden chat is not pushed.
- **Delete when:** the shell tells each frame whether it is shown.

## A chat's unread count counts its person's own messages

- **Shortcut:** `unread` counts search entries after a mark
  (cell/src/principal.rs), and `search_log` names no author.
- **Risk:** a message sent from elsewhere counts as unread.
- **Delete when:** search entries carry their principal (a schema step,
  the lists sent again), and the count leaves out the person's own.

## A busy chat's push pauses itself

- **Shortcut:** the chat's push (`notify_reply`) is a trigger, paused
  past `limits::TRIGGERED_RUNS_PER_HOUR` like any other.
- **Risk:** a chatty agent's chat stops pushing until `fragment unpause`.
- **Delete when:** a trigger's breaker counts runs that fail or loop
  (or a pause expires), or a chat's push fires once a turn.

## A chat's page not framed by its owner learns a rename at the agent's next turn

- **Shortcut:** no live event tells a page that an agent's fragment
  changed its face, so outside its owner's shell a chat page reads
  `__people` again only as a turn ends (templates/chat/site/chat.js
  `again`).
- **Risk:** others see an agent's old name until it speaks.
- **Delete when:** a fragment's pages hear its agents' face changes (an
  agent fragment's install telling the chats it is in).

## A chat's transcript is the platform's reading of the chat template's records

- **Shortcut:** `fragment_core::export::transcript_part` reads one
  template's records, a design bug by the platform rule
  (docs/cloudflare-v1.md).
- **Risk:** a change to the records' shape leaves transcripts behind.
- **Delete when:** the chat template renders its own transcript (a
  query the export asks, paged), or Paul lets the platform read the
  records' shape (then it moves to the special-case inventory).

## The effects sweep has no fault-injection test

- **Shortcut:** effects pending across a crash are settled by the next
  activation (cell/src/channels.rs `sweep`); no test crashes there.
- **Risk:** a wrong sweep loses records after such a crash.
- **Delete when:** a test build of the cell offers a fault point after
  the facet call, and an e2e kills the node there and finds the records
  after restart.

## A channel's retention is not declarable

- **Shortcut:** an app channel keeps every record, a postable one its
  newest `limits::POSTED_KEPT` (cell/src/channels.rs `append`).
- **Risk:** app channels grow without bound; long chats lose their start.
- **Delete when:** `fragment.json` can declare a channel's retention
  (count or age) under a platform ceiling, enforced and tested.

## A draft past its pace has no e2e

- **Shortcut:** `draft_api` (cell/src/channels.rs) refuses drafts past
  `limits::PRESENCE_PER_S`; no lane sends one.
- **Risk:** a change drops the pace, and nothing says so.
- **Delete when:** a lane checks that a draft past the pace is refused.

## Running runs are checked a few at a time

- **Shortcut:** the backstop checks the `RECONCILE_BATCH`
  longest-running runs per alarm (cell/src/jobs.rs).
- **Risk:** behind long sleepers, a stuck run stays `running`.
- **Delete when:** the pass takes the least recently checked runs, or
  the Workflow reports its own end from a `finally` step.

## An app's database answers to the app's own code alone

- **Shortcut:** workerd gives the supervisor no hold on a facet's
  SQLite, so `APP_DB_MAX_BYTES` and the export's `__dump` run in the
  app's own realm (cell/platform.mjs), which it can patch; the meter
  cannot see the database.
- **Risk:** unbilled, unbounded app SQLite; an export the app shaped.
- **Delete when:** the runtime lets the supervisor bound and read a
  facet's database itself, the meter samples it and the export reads
  it, with a test of a query that writes past the cap.

## A blob upload is one request, and the CLI holds whole files

- **Shortcut:** a blob is one request of at most 256 MiB
  (cell/src/blobs.rs), and the CLI reads files whole; Cloudflare refuses
  bodies over 100 MB on Free and Pro zones.
- **Risk:** large files fail whole, or cannot upload at all.
- **Delete when:** uploads go in resumable parts (R2 multipart under the
  zone's limit, tested with a 150 MB file) or by signed URL, and the CLI
  streams files from disk.

## A bad upload can delete a good copy of the same blob

- **Shortcut:** bytes that do not hash to their sha are deleted under
  that key (cell/src/blobs.rs), racing a right upload of it.
- **Risk:** an editor's bad upload empties a pointer.
- **Delete when:** uploads land under a temporary key and move to the
  content key only once verified.

## Blobs in git history lose their bytes after the grace period

- **Shortcut:** blobs no branch names for the grace period are
  collected (cell/src/blobs.rs).
- **Risk:** a rollback past it serves pointers to nothing.
- **Delete when:** someone needs old versions of large files, and a
  per-fragment policy keeps those the last N live commits name.

## Files depend on code.storage, a hosted service with no self-hosted twin

- **Shortcut:** files live in code.storage through its REST API
  (cell/src/cs.rs, cli/src/sync.rs); the only twin is the fake.
- **Risk:** not self-hostable; one outage stops every fragment's files.
- **Delete when:** a self-hostable service answers the same REST subset
  with the fake's rules as its conformance suite and the e2e passing on
  it, or files move to Cloudflare Artifacts once it has a commit API or
  reaches GA (decision 1).

## The cell signs code.storage tokens for any repo a fragment names

- **Shortcut:** a repo's id is code.storage's, so `codestorage_token`
  (cell/src/keys.rs) signs for any repo the supervisor names.
- **Risk:** a tricked supervisor gets another fragment's repo.
- **Delete when:** tokens are signed only for the repo the fragment's
  creation recorded, checked on every token, with a test.

## `fragment rollback` right after a two-step deploy picks its first step

- **Shortcut:** a diverged deploy is a restore, then a merge
  (`Promotion::RestoreThenMerge`); a bare rollback takes live's previous
  commit (cli/src/main.rs).
- **Risk:** it restores what the earlier rollback undid.
- **Delete when:** rollback's default skips a deploy's first step, or
  code.storage merges with the source's tree.

## A socket opened with a key outlives that key's revocation

- **Shortcut:** a `__live` socket re-resolves its key only at a frame
  after `LIVE_IDENTITY_MS` (cell/src/live.rs); `__watch` never does.
- **Risk:** a revoked key keeps a read-only feed it already had.
- **Delete when:** revoking a key closes the sockets tagged with it,
  with an e2e check, or every socket re-resolves its key on a timer.

## A fragment's sign-in can be forced to someone else's session

- **Shortcut:** `__signin?token=` (cell/src/auth.rs) takes any
  redemption, checking nothing the browser began.
- **Risk:** login CSRF on a fragment; no one's own data is reached.
- **Delete when:** `__signin` sets a nonce cookie the redemption must
  carry, with an e2e that redeems another browser's token and is refused.

## An app's own answers may set cookies named as the platform's

- **Shortcut:** an app's `Set-Cookie`s pass through (cell/src/js.rs
  `Facet::fetch`), `fragment_site`, `fragment_anon` and `fragview` too.
- **Risk:** only to its own fragment's visitors' sessions.
- **Delete when:** the router strips the platform's cookie names from an
  app's answer, with an e2e app that tries.

## Neither domain sends HSTS

- **Shortcut:** no answer carries `Strict-Transport-Security`.
- **Risk:** a first visit can be held to http; no cookie is sent there.
- **Delete when:** both zones send HSTS, checked by the hosted lane.

## Preview cards drive Browser Rendering over a CDP client of our own

- **Shortcut:** a card's shot (cell/src/card.rs `take`, `Cdp`) drives a
  session to hear the page's errors (`card::Heard`), which a quick
  action does not return and `wrangler dev` cannot run.
- **Risk:** a change to the session routes fails every shot.
- **Delete when:** a quick action reports what the page said as it
  loaded (or the report comes from elsewhere), runs locally (or a fake
  stands in), and the cell's compatibility date reaches 2026-03-24:
  then `take` is one `quickAction("screenshot", …)`, and `Cdp`, the
  session and `billed_ms`'s keep-alive go.

## Two imports into one session archive at once agree last-writer-wins

- **Shortcut:** `fragment session import` commits each batch of
  conversations as upserts on whatever head main has
  (cli/src/session/mod.rs `commit`); only the indexes' commit is made
  again from main when it loses its race.
- **Risk:** two imports of one conversation at once keep the last one's
  file, and its index entry may name the other's copy, so the next
  import skips a newer copy once or writes one again. Nothing is lost.
- **Delete when:** each batch's commit is made again from main as the
  indexes' is (keeping the newer entry), or the archive moves to an app
  whose mutation writes both.
