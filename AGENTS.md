# fragment

Fragment is the product: stateful, multiplayer web apps published from
the CLI. Agents are fragments a computer of your own runs (today, Hermes
profiles; docs/computers.md).
Built on fragment (this repo carries fragment's full history; its
TypeScript runtime was cut in phase 2; this repo is
github.com/finitecomputer/fragment's `master`, the Cloudflare line since the
cut: fragment.club runs the `celld` branch, tag `celld-final`, until
cutover, decision 35). The cell runs on Cloudflare Workers (workerd under
`wrangler dev` locally) since phase 2 of that plan. The cut (decision 33)
took Hermes, computers, the desktop, the personal agent's chat,
sandcastle, and the fleet's deploy path off master; they live at the tag
`celld-final`. Computers came back in its phase 4, Hermes as their
image's agent runtime.

## Read first

0. `docs/cloudflare-v1.md` — **the plan of record since 2026-10-02**:
   fragment v1 entirely on Cloudflare, its decisions, phases and the
   hard cut. Where it disagrees with anything below, it wins.
   `SPECIAL-CASE-INVENTORY.md` lists the platform surfaces that are not
   fragments, and why; keep it short.
1. The decisions are cloudflare-v1.md's: its own, and the ROADMAP's that
   still hold, as R4 to R18. The truth map every change is checked
   against is MODEL.md's ("Where each fact lives").
2. `docs/MODEL.md` — the core model, in Cloudflare's primitives.
   `docs/api.md` — the wire contract the cell answers.
   `docs/computers.md` — the platform's contract with a computer's image;
   `docs/bridge.md` — the bridge, the one process an image runs between
   its agent runtime and the fragment API;
   `docs/chat-records.md` — a chat's records, which the bridge writes
   and the chat template reads;
   `docs/durable-computers.md` — how a computer's state survives a
   sleep, a crash or a rolled-back disk: the design of record;
   `docs/ledger.md` — the usage ledger: what is metered, at what price,
   and who pays;
   `docs/billing.md` — seats, orgs, Stripe, trials and the operator's
   admin (decisions 51 to 59);
   `docs/finite-integration.md` — how fragment will move into
   finite.computer (Finite V3): every Core concern, its stand-in here,
   and the swap; update its row with any change that touches one;
   `docs/hermes-relay.md` — Hermes' Relay contract, read from its code
   (its connector went at the cut).
3. `docs/published-fragments.md` — primitives that must stay expressible.
   `docs/secrets.md` — where secrets live and how code reaches them.
4. `docs/technical-debt-ledger.md` — the only place shortcuts may live.
5. The engineering style: `/Users/futurepaul/dev/finite/engineering-style/
   engineering-style.md` (hard cuts, limits, assertions on in release,
   typed errors, valid/invalid/replay/restart tests, Rust for tooling).

## Commands

The cell (`cell/`, Rust on Workers), the CLI (`cli/`), and the harness
(`xtask/`, `crates/`). Tooling is Rust, and JavaScript's own tools come
through `package.json` on xtask's pinned Node ("Choosing tools", below);
the repo has no shell or Python tooling. Its JavaScript from npm is
wrangler and the Sandbox SDK the cell's shim bundles, pinned in
`package.json`; every page's script is first-party source.

- One-time setup: `cargo install worker-build --version 0.8.5 --locked`,
  `rustup target add wasm32-unknown-unknown`, an LLVM clang that compiles
  for wasm32 (Apple's does not: `brew install llvm`, or Nix, which xtask
  finds; or name one with `CC_wasm32_unknown_unknown` and
  `AR_wasm32_unknown_unknown`), and Docker. No Node install and no `npm
  ci`: xtask (and the e2e) bring their own. On first use they fetch the
  pinned Node (v24.22.1, `crates/devstack/src/node_release.rs`) from
  nodejs.org/dist into `target/tools/node-v<version>-<platform>/`,
  checking the tarball's SHA-256 before unpacking it, and run that Node's
  own npm (`npm ci`) whenever node_modules is missing or was installed
  from another package.json, package-lock.json or Node. Every JavaScript
  process (npm, wrangler as `<node> node_modules/wrangler/bin/wrangler.js`,
  and whatever they start) runs on it, with it first on PATH; no `node`
  from PATH is ever used, and no other Node. Their caches are the repo's:
  `XDG_CACHE_HOME=target/cache` (miniflare keeps the Chrome for Testing
  that preview cards' local Browser Rendering downloads, 126, about 145
  MB, in `target/cache/.wrangler/chrome`), `WRANGLER_CACHE_DIR=
  target/cache/wrangler`, and npm's in `target/cache/npm`; nothing goes
  to the system's cache. Moving the pin: docs/pinned-node.md.
- `cargo xtask build`: the cell for wasm32, held to its budget
  (`CELL_WASM_MAX_BYTES`, 6.5 MB raw, xtask/src/build.rs: it fails over
  it, saying so), and the release's files (the templates, the shell's
  files, the agent docs) into `cell/build/assets`, each named by its
  SHA-256: the Worker's Static Assets, which the cell reads by the index
  it carries (cell/src/assets.rs; docs/api.md, The release's files), a
  deploy uploads, and the e2e's staged copy holds. A file the cell
  serves belongs there, not in an `include_bytes!`. `cargo xtask build
  --dev` is `cargo xtask dev`'s build (its first, and `wrangler dev`'s
  rebuild, cell/wrangler.jsonc): the same source, flags and files, in
  cell/Cargo.toml's `devstack` profile (the release's without fat LTO
  into one codegen unit, incremental) and without wasm-opt, so a change
  to a file of the release (the shell's, a template's) rebuilds in
  seconds, a Rust change in about half a minute; its Wasm, about 7.7 MB,
  is held to no budget. Each build marks `cell/build/built` with its
  profile and its Wasm's SHA-256 (crates/devstack/src/built.rs), and a
  deploy and the e2e's staging refuse any build but the release's,
  finished. A build holds `cell/build.lock` alone from its start to its
  mark; the e2e's staging (while it copies) and a deploy (through every
  wrangler try) hold it shared, so a rebuild waits for them, and they
  for it.
- `cargo xtask check`: every first-party JavaScript file (`.js`, `.mjs`,
  `.cjs` git tracks or would add, less the vendored ones) through `node
  --check` on the pinned Node, each as a module or a classic script as
  it is loaded (`xtask/src/js_syntax.rs`: its `VENDORED` and `CLASSIC`
  lists), naming each file and line that does not parse; that the
  schema lock (`crates/core/migrations/schema.lock`) is what the
  platform Durable Objects' steps make (`xtask/src/schema_lock.rs`); then
  host tests and clippy (host and wasm), warnings denied.
- `cargo xtask schema-lock`: appends the Durable Objects' new schema
  steps, and what they make, to the lock; it refuses to change or drop a
  locked step. A schema changes only by a new step (docs/MODEL.md,
  Schemas; `crates/core/src/migrate.rs` says how).
- `cargo xtask e2e [--only <section>[,...] | --except <section>[,...]]`:
  builds `cell/` and the CLI, then runs `crates/e2e` against
  a fresh `wrangler dev` node (workerd) and the in-process fakes, which
  stand only at vendor boundaries (sections, in order:
  auth, create, lockdown, keys, members, identities, signin, levers, secrets,
  delegation, files, deploy, templates, share, faces, isolation, frames, ops, public,
  effects, facet-cap, app-lockdown, site, watch, schemas, channels,
  live, routes, cli, browser, jobs, triggers, appfiles, blobs, notes,
  brain, doc, push, ai, ledger, transcribe, finite-private, orgs, billing,
  billing-page, trials, admin, admin-page, shell, computers, chat,
  shell-ui, wipe, hermes, agent-smoke, agent-ask, agent-app, agent-restart,
  bot-mode, checkout, sync, migrations, restart;
  `crates/e2e/src/lanes/mod.rs`). `finite-private` starts its own node
  shaped as Finite's deployment (`cheap` on Finite Private, `medium` on
  OpenRouter with its provider routing, both the model fake's
  OpenAI-compatible face; decision 61), then the plain one again.
  `wipe` wipes a person it made (docs/api.md, Operators) with an
  operator key no person holds: the local node's own, or, hosted, the
  file `--operator-key-file <file>` names (its npub in the config's
  `operators`; a skip without one).
  `computers`, `chat` and `shell-ui` run the stub image (`images/stub`)
  in Docker, and `chat`, `frames` and `shell-ui` drive Chrome; `hermes`, the real-Hermes lane, builds
  and runs our Hermes image (3.8 GB), so it runs only by name
  (`--only hermes`) and is a skip otherwise. A check local workerd cannot make (its
  CPU and memory limits, a Workflow that sleeps through a crash) is a
  `skip`, printed and counted: the hosted lane's. The share, isolation, browser, and
  notes sections drive headless Chrome (`CHROME_BIN` to choose one; one
  Chrome serves the whole run, a fresh browser context per section;
  frames, and computers' frame checks, start one of their own that
  blocks third-party cookies, as Safari does);
  `--only triggers` waits for a cron
  minute (up to a minute; a full run deploys its cron fragment sections
  earlier). A section that errors or panics is one FAIL and the sections
  after it still run. The node runs from a staged copy of the cell in the
  run's own scratch (`target/e2e/<run>/cell`), so the e2e and `cargo
  xtask dev` can run at once, as can runs in several worktrees: a node's
  computer images are its project's own (each its Dockerfile plus a
  `dev.fragment.project` label, written under `<project>/.wrangler/images/`),
  so another node's teardown, which removes containers by image, never
  removes its computers; and a run removes the containers its nodes left
  (each computer's and its `-proxy` sidecar, named for the computers in
  its state, never another's) when it ends, fails, panics, or gets Ctrl-C,
  SIGTERM or SIGHUP, after killing its node (`crates/devstack/src/
  containers.rs`; `xtask dev` does so when wrangler exits). That scratch holds each node boot's log
  (`node-<port>-<boot>.log`, wrangler's debug logs on) and is removed when
  every check passes, kept when one fails (`FRAGMENT_E2E_KEEP=1` keeps it
  anyway). Each section declares what it needs (`crates/e2e/src/needs.rs`:
  fakes, the node, the whole deployment, levers, local Docker, Chrome,
  computers, models, two sites), and that alone chooses the hosted set.
  The node's test levers (`/api/test/*`) take a secret made per run.
  A wait that runs out its limit prints `(a wait ran out its …s at
  <file>:<line>)`: one that does so on a passing run costs every run.
  A call that fails says when it was sent (UTC, as the node's logs stamp
  their lines) and how long it waited: a dropped connection fails at
  once, a hung call after its wait. A boot's log can lack its last
  seconds when a failure stops the node at once (the Workers' output
  reaches it late, in bursts): logs that just stop are not a node that
  died. The run's client never reuses a connection idle 4 s: workerd
  closes one idle 5 s, and a request written onto it as it closes is
  lost (`POOL_IDLE`, `crates/e2e/src/api.rs`).
- `cargo xtask e2e --shard <k>/4`: one of the four shards CI runs, each
  on its own runner with its own build and node (`SHARDS` in
  `crates/e2e/src/lanes/mod.rs`: every section in exactly one, a host
  test checks; rebalance it from the time each shard's log prints for
  each section). CI's `e2e` job is green when every shard is. CI splits
  each shard's run in two, `--build-only` then `--no-build`, so the
  cache saves between (the build also builds the computer images ahead
  of the node, beside the Rust: `xtask/src/build.rs`).
- `cargo xtask e2e --hosted --config <deploy config> --branch <b> [--only
  … | --except …] [--dry-run | --sweep [<run>] | --sweep-all]
  [--max-paid-calls <n>] [--operator-key-file <file>]`: the hosted
  lane, the same sections against the branch deployment
  `https://<b>.<zone>` on its real vendors (crates/e2e/src/hosted.rs). Its
  people sign in through the preview's levers as `<name>@e2e.test` (the
  config's `test_secret_file`, read from its file; docs/secrets.md), its
  fragments are `e2e-<run>-…` (the run's id, 10 hex digits: its clock's 6
  and 4 random, printed as it starts), and a section that needs what a preview lacks is
  a skip that says why. Paid calls (models, AI steps) are lent from the
  run's budget (default 60), each person's capped by their ledger, and the
  run ends saying what it spent. `agent-smoke` runs only here, and only
  by name (`--only agent-smoke`): a real agent (our Hermes image on its
  real model, `Need::RealAgent`) and Chrome, through the flows a person
  uses (a first reply, the CLI, an app, the desktop and its screen, a
  command that asks no one, a sleep and a wake), for about half an hour
  and up to 50 paid calls (crates/e2e/src/lanes/agent_smoke.rs).
  `agent-app` runs only here too, by name: a real agent asked in words
  for a shared grocery list, which it makes and deploys; then Chrome opens
  it as its owner at its own origin, in two tabs, and reads what the page
  did from its DevTools events: it loads, throws and logs nothing (a card
  shot's signals), its live socket says hello, an item typed in one tab
  shows in the other, and its own query answers; a failure says what the
  page did ("the page threw: …"). Up to half an hour and 30 paid calls
  (crates/e2e/src/lanes/agent_app.rs). The `hermes` section runs the same
  check on the app its Hermes makes on the scripted model (the todo
  template: fixed, so a failure is the platform's).
  `agent-restart` runs only here too, by name: its owner's Restart of a real Hermes computer awake
  after a reply (its newest save restored, fresh from the image, asked
  twice made once), then a reply after it; up to 10 paid calls (16 with
  a candidate) (crates/e2e/src/lanes/agent_restart.rs). `checkout` runs only here, by
  name and with `--operator-key-file`: a guest buys the $200 seat on
  Stripe's own sandbox Checkout in Chrome (card 4242…), comes back to it,
  and their first agent answers; about a minute, one paid call
  (crates/e2e/src/lanes/checkout.rs). `--candidate <image>` (one of the
  config's images, not its default) pins each real-agent section's new
  computer to that image before its first start, and makes
  `agent-restart`'s Restart the upgrade and then the rollback: a canary
  round's run (docs/computers.md, "Stable and canary"). `--dry-run`
  prints the plan (base URL,
  what runs, what is skipped and why) and calls nothing. A preview is
  shared (several sessions run on it at once), so a sweep is one run's
  unless told otherwise: `--sweep` deletes the fragments of the last run
  that finished in this checkout, `--sweep <run>` those of the run it
  names, whatever their age and no other run's, and puts that run's
  people's computers to sleep (not one whose person owns another run's
  fragment too); a hosted run ends naming its id for this. `--sweep-all`,
  for when nothing else runs there, deletes every e2e fragment at least an
  hour old and spares younger ones (a run started meanwhile may be using
  them), and their people's computers. Each says what it kept and why:
  other runs' fragments counted by run, young ones named with their age
  (crates/e2e/src/hosted/sweep.rs). `cargo xtask e2e --rehearse` keeps the
  hosted lane's rules on the local node (shaped as a branch, its fakes
  hidden from the lanes) and ends with its sweeps: its run's, which keeps
  another run's fragment made beside it, then the whole one's, which
  spares it for its age. Running it against a preview spends test cents:
  Paul's or the coordinating session's to run.
- `cargo xtask dev [--clean] [--real-agent [--models <config>]]`: the dev stack in the foreground under
  `wrangler dev`: the cell on :8790 with fragments at
  `http://<name>.fragment.localhost:8790/`, which rebuilds (`build
  --dev`, above: seconds for the shell's and the templates' files)
  when `cell/src`, `cell/shell`, `crates/`, `templates/` or `cli/`
  change, the model route and AI steps on
  the Workers AI fake on :8796 (echoes, and draws placeholder JPEGs for
  image steps; dev never calls a real model), which spend their payer's
  ledger (dev people are seats, with the month's included credit), the
  code.storage fake on :8792 (state in `target/devstack/`; its org
  key and the host secret are made there on first run), and sign-in at
  http://127.0.0.1:8790/ through the WorkOS fake on :8794 (any email), or
  a real WorkOS environment when `WORKOS_CLIENT_ID_FILE` and
  `WORKOS_API_KEY_FILE` name its files, and the platform's mail to the
  mail fake on :8798, which prints each message (dev never sends real
  mail). Its secrets are seeded into
  wrangler's local Secrets Store in `cell/.wrangler/state` and bound by
  name as a deploy binds them (`--clean` clears them with the state;
  docs/secrets.md). Each boot's log is
  `target/devstack/node-8790-<boot>.log` (printed at start;
  `FRAGMENT_NODE_LOGS=1` adds wrangler's debug logs). Point the CLI at it with
  `FRAGMENT_HOST=http://127.0.0.1:8790` and run `fragment login` once. Dev fleets let jobs
  fetch local addresses (`FRAGMENT_EGRESS_LOCAL=allow`). `--real-agent`
  runs new computers on our Hermes image, built from the tree (its first
  build takes a while), and the text tiers on real models: a deployment
  config's `models` (deploy/example.jsonc's, or `--models <config>`'s: a
  copy of the preview's runs the preview's), checked as a deploy checks
  them, whose OpenAI-compatible slots call with the one key in the file
  `FRAGMENT_DEV_MODEL_KEY_FILE` names; its Workers AI slots stay on the
  fake (xtask/src/dev_agent.rs). The node lists an operator key of its
  own (`target/devstack/operator-key`, made on first run), so `fragment
  operator wipe <email> --yes --key-file target/devstack/operator-key`
  wipes a person there; signed in again, they are a new person, whose
  first run makes a fresh agent: a fresh agent's test, as on a preview
  with an operator key its config lists.
- `cargo xtask try <todo|inbox|notes|doc> [name]` (with `cargo xtask dev` running):
  creates and deploys a fragment from a template under
  `target/devstack/try/` (never in the repo) and prints the link to open,
  a curl for the inbox, and a `fragment` alias for the dev stack. The
  reference templates: `todo` (operations, channels, the browser
  library), `inbox` (a trigger, a job, the inbox), and `notes` (files as
  the state, read through `App.fetch`, refreshed by a file trigger).
  `fragment new|init --template` scaffolds any of `templates/` (also
  `blank`, `calories`: a channel trigger and a text step, and `doc`:
  paragraph notes, a mixin in `applib/` any app can take), and
  `fragment create --template` makes one on the platform; the shell offers
  no templates: its dock's Add teaches how to ask an agent for an app.
- `cargo xtask secret set <name> --config <file> [--from-file <path>]`,
  `secret gen <name> --config <file>`, `secret list --config <file>`: the
  deployment's secrets in its account's Cloudflare Secrets Store
  (xtask/src/secret.rs, through wrangler on the pinned Node). `set` takes
  the value from wrangler's hidden prompt, standard input, or a file once,
  and updates a secret already there (never the config's host secret,
  which rotates by name); `gen` makes a new one of 32 random bytes as hex;
  `list` shows names and times and what the config names that the store
  lacks. `set` and `gen` make the account's one store when there is none.
  No `rm`. `--local <state dir>` acts on wrangler's local store instead.
  Setting a remote secret is Paul's (or the coordinating session's).
- `cargo xtask stripe check|setup --config <file> [--branch <name>]
  [--webhook-secret-file <path>]`: the deployment's Stripe as
  docs/billing.md wants it, on the account its config's `stripe.key_file`
  names (xtask/src/stripe.rs): `check` reads its prices (by lookup key),
  its own portal configuration and its webhook endpoint, and says what is
  wrong; `setup` makes what is missing (an endpoint's signing secret to
  the file, 0600, never printed, for `secret set`). Live mode is Paul's
  to run. A branch deploy whose config names `stripe.key_file` makes the
  branch's own endpoint each time and uploads its secret as a Worker
  secret beside the test secret (decision 58).
- `cargo xtask deploy --config <file> [--branch <name>] [--pinned-only]`:
  builds and deploys to Cloudflare from a deployment's config, kept
  outside the repo (`deploy/example.jsonc`; xtask/src/deploy.rs). It
  lists the store first and refuses, before anything is built, when a
  secret the config names is missing (naming the `secret set` for each);
  then it waits, 20 minutes at most, for every image the config pins to
  be ready on Cloudflare's network (the Containers API's image
  preparation, asked with `CLOUDFLARE_API_TOKEN`: xtask/src/prepare.rs),
  and refuses, with nothing built or changed, an image the account
  cannot prepare or one still pending then (wrangler's own wait gives up
  at 15 minutes); it binds them by name and uploads no secret but a branch's test
  secret, and runs `wrangler deploy` again (three tries at most, 10 s
  apart) only for a failure a re-run answers: its Container settings not
  applied, or an image's push to the registry timing out. A branch is a
  complete copy at `<branch>.<zone>`, its fragments at
  `<name>--<branch>.<zone>`. `--pinned-only` (CI's)
  refuses, before anything is read or built, a config with a computer
  image built from the tree (a `dockerfile`), naming it: every image is
  kept at its digest, and the deploy needs no Docker. `cargo xtask
  teardown --config <file> --branch <name>` removes one (irreversible:
  ask Paul).
- `cargo xtask image push hermes --repository <name> --tag <tag>
  [--summary <file>]`: builds the Hermes image as a deploy builds it and
  pushes it to the Cloudflare registry of the account
  `CLOUDFLARE_ACCOUNT_ID` names (wrangler's `containers push` on the
  pinned Node, with `CLOUDFLARE_API_TOKEN`; up to four tries, as its
  base's layer is 1.4 GB), then prints the reference a config pins it at
  (`registry.cloudflare.com/<account>/<repository>@sha256:<64 hex>`,
  read from the registry's digest and held to deploy.rs's `pinned_ok`)
  and its config line (xtask/src/image.rs). `cargo xtask image copy
  hermes --from <reference> --repository <name> --tag <tag>` builds
  nothing: it copies an image another account's registry holds (the
  source's account and token in `FROM_CLOUDFLARE_ACCOUNT_ID` and
  `FROM_CLOUDFLARE_API_TOKEN`; `<account>` may stand for it in the
  reference) byte for byte, through skopeo (`--preserve-digests`, each
  side's login minted by wrangler, in a file of its own: both are
  registry.cloudflare.com, where one Docker login holds one), and
  refuses a copy whose manifest is not the source's digest. The Image
  workflow runs both (below).
- Crates: `crates/proto` (wire types), `crates/core` (the cell's pure
  logic, host-tested; sealing at rest is `seal.rs`), `crates/nip98`,
  `crates/templates` (the release's files: `templates/`, the shell's,
  the agent docs; an index of them in the cell, their bytes with
  `embed`, for the CLI and xtask),
  `crates/fakes` (code.storage, Workers AI, WorkOS, Email Sending, a push service),

  `crates/devstack`, `crates/e2e`.
- `images/` (the computer images: the bridge, the stub, our Hermes image;
  docs/bridge.md) is its own workspace: `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` there (CI:
  `.github/workflows/images.yml`); `cargo test -p fragment-bridge --test
  docker -- --ignored` builds both images and runs them in Docker
  (linux/amd64) against a fake API and a scripted model, real Hermes
  included (CI: images.yml's `docker`, below). The e2e's computer
  sections run the stub image under `wrangler dev`, which needs Docker.
- `.github/workflows/ci.yml` runs `check`, the e2e's four shards (`e2e
  shard k/4`), and `e2e`, green when they all are, on Linux, and then, on
  a push to master or `production` that both are green for, `deploy`
  (below). A push whose tree a green run already tested skips the e2e's
  shards and deploys once `check` is green (about two minutes: it holds
  the schema lock to what the push moved from, which no earlier run can
  prove): its first job, `proven`, asks the API. Each run's
  `e2e` job records the tree its shards tested (an artifact,
  `tested-tree-<tree>`). On `production`, a run of ci.yml that succeeded
  on a push to master of that SHA, or of a commit with that tree, proves
  it, its preview deploy included (Paul, 2026-10-20); on master, such a
  run, or the green run of the pull request a merge commit merges (its
  head the commit's second parent) that recorded the merge commit's
  tree: a pull request's run tests its merge into master as master
  stood, so this holds when master has not moved since (Paul,
  2026-10-23). Its caches restore on
  every run and save only from master's pushes (each one's contents are
  named in the workflow): `check`'s Rust dependencies and the e2e's
  tools through `Swatinem/rust-cache`; the e2e's builds, our own crates'
  with them, through `actions/cache`, one entry per master commit with
  a record of the tree it was built from, from which `xtask/src/stamp.rs`
  (compiled alone, before any cargo build) sets a checkout's mtimes so
  cargo rebuilds only what was not built from its very content; `prune`
  deletes the older entries); the stub image's layers through BuildKit's
  Actions cache, built before the e2e's build so it and the node's boot
  find them; the pinned Node's and the JavaScript caches through
  `actions/cache`. A push's run is never cancelled by a newer
  one, which waits (a pull request's is). `images.yml` runs `images/`'s
  `check` on every change there, and `docker`, the Docker lane above
  (both images built, then every `--ignored` test of docker.rs on them,
  in parallel; about 13 minutes), on pull requests and master's
  pushes that touch images/hermes, images/bridge, images/stub, the
  images' manifest or lock, or the workflow (its `changes` job, through
  `dorny/paths-filter`), and on a run by hand. It keeps no layer cache: the workflow
  says why. `release.yml` builds the CLI for macOS and Linux.
  `image.yml`, the Image workflow, runs only by hand (Actions, image, Run
  workflow: `environment` preview or production, `from`, `tag`) in that
  GitHub environment, for its `CLOUDFLARE_API_TOKEN` and
  `CLOUDFLARE_ACCOUNT_ID` (required there), from the branch that deploys
  it (master for the preview, `production` for production; any other
  is refused), to the repository the deploys made
  (`fragment-preview-computer-hermes-canary`,
  `fragment-computer-hermes`). Without `from` it builds and pushes
  (`cargo xtask image push`; `tag` empty for the commit's first 12 hex
  digits): the preview's canary, or, warned that its digest is not the
  canary's, production's. With `from`, production's alone, a proven
  preview reference, it copies it (`cargo xtask image copy`, with
  production's `PREVIEW_REGISTRY_TOKEN` and `PREVIEW_ACCOUNT_ID`; `tag`
  empty for the digest's first 12 hex digits): a promotion keeps the
  digest the round proved (Paul, 2026-10-20). Its summary names the
  digest and the `computers.images` line to pin it with, the account
  masked.
- CI deploys to Cloudflare (ci.yml's `deploy`: `xtask deploy
  --pinned-only`) on every green push: master's to its preview,
  `preview.finite.build` (`--branch preview`, a branch copy on the dev
  zone), and the `production` branch's to production (`finite.chat`, its
  fragments on `*.finite.place`, in Finite's production account: Paul,
  2026-10-16). Only Paul updates `production` (protected: no force push,
  no deletion): his push, or the pull request he merges into it, is the
  approval of a production deploy. Promoting a commit master's run has
  proved green deploys in minutes; anything else (a hotfix pushed
  straight to `production`, a commit whose master run is red or still
  running) runs the full tests first, about 20 minutes. Each deploys
  from a GitHub environment, `preview` or `production`, that only its
  branch may use,
  holding the variable `DEPLOY_HOST` (its platform's host, whose
  `/healthz` must then answer the commit) and the secrets
  `DEPLOY_REPO_TOKEN` (a fine-grained token with Contents · Read on
  finitecomputer/fragment-deploy alone),
  `CLOUDFLARE_API_TOKEN` (wrangler's, and the DNS token), the preview's
  `PREVIEW_TEST_SECRET`, and, optional but recommended,
  `CLOUDFLARE_ACCOUNT_ID`, named only so the job's logs mask the account
  wherever wrangler or xtask print it. The configs live in the private
  repo finitecomputer/fragment-deploy, `preview.jsonc` and
  `production.jsonc` at the root of its `main`: names and paths, no
  secret's value, kept out of this public repo's logs and readable by
  the team. The job checks out its environment's at `main` as it stands
  then, prints that commit (never the contents), and deploys from it,
  so a change there deploys at the environment's next deploy: the
  preview's at master's next green push or a re-run of the `deploy` job
  of master's latest ci run, production's only at a deploy of Paul's
  (his next move of `production`, or his re-run of its `deploy` job).
  The job writes the token and the test secret where a CI config names
  them, `~/.config/fragment/secrets/cloudflare-api-token` and
  `…/preview-test-secret` (deploy/example.jsonc). Every image a CI
  config names is pinned by digest: a canary is built and pushed by the
  Image workflow (above), production's stable copied by it from the
  preview's registry, and the digest its summary names then pinned by a
  commit to the environment's file in fragment-deploy
  (docs/computers.md, "One round"). Other branches' copies deploy by
  hand.
  fragment.club (the celld fleet on Fly) deploys only from the `celld`
  branch (tag `celld-final`) until cutover: its fleet file, node image
  and docs/operate.md's commands are that branch's. Deploys to the
  hosted fleet, production deploys by hand, and DNS changes are Paul's
  to approve.

## Rules

- The deployment's secrets live in its account's Cloudflare Secrets
  Store: set with `cargo xtask secret`, named in its config, bound to the
  Worker by name, and read only by `cell/src/keys.rs` (docs/secrets.md).
  Only the DNS token and a preview's test secret are files read by path.
  Never print them, pass them on a command line (no `--value`), or commit
  them; deleting one is Paul's.
- The configs CI deploys from live in the private repo
  finitecomputer/fragment-deploy (`preview.jsonc`, `production.jsonc`):
  names and paths, never a secret's value (a secret is named there by
  its store name). A change to one is a commit there, which its
  environment's next deploy takes (the CI bullets above); none is kept
  in a GitHub secret.
- `cargo xtask dev` runs `wrangler dev` on `cell/`, which rebuilds when it
  changes; the e2e runs a staged copy (`target/e2e/<run>/cell`) with its own
  variables, state and dev registry, so the two can run at once.
- A change to a call's shape between the Worker and a Durable Object (or
  between two objects) ships safely: every call carries its caller's
  release (`FRAGMENT_DEPLOY_ID`, the deploy's commit) and the object
  refuses another release's (503 `deploying`) before it acts, so callers
  and callees of different releases refuse each other during a deploy and
  the caller asks again (docs/MODEL.md, Calls between releases). Call an
  object only through `routed::send`/`send_again` (xtask's guard). What
  outlives a deploy (a queue message, a stored row) is not a call: its
  shape stays readable by the next release.
- The remote is `fragment-rs`, github.com/finitecomputer/fragment: work goes
  up as a branch and a pull request against `master` (branch names under
  `ci/` are refused). The coordinating session reviews and merges a PR
  once its CI is green, which deploys it to the preview. Only Paul
  updates the `production` branch. Other deploys, adding a remote,
  deleting Sprites, DNS, and anything else irreversible are Paul's to
  approve.

## Choosing tools

Paul, 2026-10-19, after a review of the code we wrote ourselves where
something off the shelf exists:

- **Prefer what already exists.** When an idiomatic Cloudflare feature, a
  standard crate or a well-kept library simplifies the stack, use it and
  delete ours. Code of our own needs a reason a reader finds beside it
  (the cell's wasm budget, a Workers limit, a security property): in a
  comment, a decision, or a debt-ledger entry.
- **A build step is acceptable when it buys significant simplification or
  safety:** a type checker, a linter, a bundler for code that would
  otherwise be copied by hand. Pages are served as the source in git
  today. JavaScript's tools come through `package.json` on xtask's pinned
  Node, as wrangler does, never from a developer's own install.
- **One concern per pull request.** A fix for one finding, one swap, one
  convergence: separable changes go up separately, so each stays small
  to review and to revert.
