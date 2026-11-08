# fragment

Fragment is the product: stateful, multiplayer web apps published from
its CLI. The cell runs on Cloudflare Workers. Agents are fragments run
by a computer of their owner's; Hermes is an image's runtime, not a
platform concept.

## Read first

- `THE-FRAGMENT-WAY.md`: Paul's words on what Fragment is for and how it
  should grow. Human-written; an agent never edits it.
- `docs/cloudflare-v1.md`: binding product decisions and the platform
  rule. `SPECIAL-CASE-INVENTORY.md`: surfaces that are not fragments,
  and why; keep it short. New runtime, template or vendor knowledge
  belongs in an image, template or skill, not the platform.
- `docs/MODEL.md`: the authority map ("Where each fact lives").
- Contracts: `docs/api.md` (wire), `docs/computers.md` (image, durability),
  `docs/bridge.md` (bridge), `docs/chat-records.md` (chat records),
  `docs/secrets.md` (secret custody), `docs/ledger.md` (usage),
  `docs/billing.md` (orgs and Stripe).
- `docs/published-fragments.md`: primitives that must stay expressible.
- `docs/technical-debt-ledger.md`: the only place shortcuts may live;
  include the condition for removing each one.
- Engineering style: `/Users/futurepaul/dev/finite/engineering-style/engineering-style.md`
  on the Mac: hard cuts, limits, release assertions, typed errors, and
  valid/invalid/replay/restart tests. Tooling is Rust.

## Where things live

- `cell/`: Rust Worker and JS shims; `cell/shell/`: platform pages.
- `cli/`: CLI; `xtask/`: build, check, dev, e2e, deployment tooling.
- `crates/proto`: wire types; `crates/core`: pure logic and state machines;
  `crates/nip98`: authentication; `crates/templates`: release assets;
  `crates/fakes`, `crates/devstack`, `crates/e2e`: local vendor fakes and harness.
- `templates/`: fragment templates; `applib/`: reusable app code.
- `images/`: a separate Rust workspace (bridge, stub, Hermes).
- `bench/search/`: the benchmark behind each fragment's search index
  (`cell/src/search_index.rs`, FIN-310), a separate Rust workspace
  (`cargo test --release`). The LanceDB service it also measured is the
  tag `search-service-lancedb`.
- `.github/workflows/`: CI, image promotion and CLI releases. Read the
  relevant workflow and xtask module for mechanics and flags.
  Released CLIs warn on stderr when finite.chat advertises a newer
  published version (`cli/src/notice.rs`).

## Commands

Setup: `cargo install worker-build --version 0.8.5 --locked`,
`rustup target add wasm32-unknown-unknown`, Docker and an LLVM clang that
supports wasm32 (Apple's does not; `brew install llvm`, Nix, or
`CC_wasm32_unknown_unknown` / `AR_wasm32_unknown_unknown`).

- `cargo xtask build`: release Wasm and assets, enforcing the Wasm budget.
  `cargo xtask build --dev`: the faster development build.
- `cargo xtask check`: source guards, JavaScript syntax, schema lock,
  host tests and host/wasm clippy with warnings denied.
- `cargo xtask schema-lock`: append new schema steps to the lock.
  Never edit or drop a locked migration; add a step instead.
- `cargo xtask wire`: write the wire's schema (`docs/wire`) and lock
  (`crates/proto/wire.lock`) from `crates/proto`'s routes and types.
  Check fails on a change an older client would feel; retire a fact in
  `crates/proto/wire.retired` only once its clients are gone.
- `cargo xtask dev [--clean] [--real-agent [--models <config>]]`: dev stack
  at :8790, fragments at `http://<name>.fragment.localhost:8790/`.
  Default vendors are fakes. `--real-agent` calls real text models using
  `FRAGMENT_DEV_MODEL_KEY_FILE`. CLI: set `FRAGMENT_HOST=http://127.0.0.1:8790`
  and run `fragment login`.
- `cargo xtask try <todo|inbox|notes|doc> [name]`: deploy a scratch app
  while dev runs. `fragment new|init|create --template` for other templates.
- `cargo xtask e2e [--only <section>[,...] | --except <section>[,...]]`:
  build and test on fresh workerd with fakes; Docker/Chrome as needed.
  Section definitions: `crates/e2e/src/lanes/mod.rs`; prerequisites:
  `crates/e2e/src/needs.rs`. Real-Hermes tests run only by name.
- `cargo xtask e2e --hosted --config <file> --branch <b>`: real vendors;
  `--dry-run` prints the plan. Paid calls need Paul's or the coordinating
  session's authorization. Sweeps delete remote state; inspect scope first.
- In `images/`: `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`.
  `cargo test -p fragment-bridge --test docker -- --ignored` builds and
  tests the real images with Docker.
- Deployment commands: `cargo xtask secret set|gen|list`, `stripe check|setup`,
  `deploy --config <file> [--branch <name>]`, `image push|copy`.
  Read `xtask/src/main.rs` and the module before running them.

xtask supplies its pinned Node and npm; don't install Node or run system
`npm ci`. All JavaScript tools come through `package.json` on that Node,
never a developer's install. Pin changes: `docs/pinned-node.md`.
A file the cell serves belongs in the release assets built by
`crates/templates` into `cell/build/assets`, not `include_bytes!`.
Dev and e2e use separate state; failed e2e logs remain in `target/e2e/`.

## Proving your work

You prove your own work before anyone merges it. CI on a pull request is
not the safety net, and nobody waits on bot reviews. This machine runs the
tests faster than CI does, so run them here. Work is ready only when you
can show evidence that it does what it claims and breaks nothing else.
"It compiles" and "the tests I wrote pass" are not evidence.

Put an evidence pack in the PR body and in your report:

- **What and why:** what changed and why, in plain words. Say whether it
  touches the wire, stored rows or money.
- **What ran:** each command with its result and counts, on the final head
  merged with current `master`:
  - `cargo xtask check`.
  - `cargo xtask e2e --only …` for every section that covers a changed file.
    Grep `crates/e2e` for each file and run every lane that uses it.
  - For `images/` or the bridge: `cargo test --workspace` and the Docker
    tests (`cargo test -p fragment-bridge --test docker -- --ignored`).
  - For iOS: `xcodebuild` tests on the Mac, in a build folder of your own,
    on a simulator chosen by id.
  - For the cell: the Wasm delta against the exact `master` you merged,
    both built in one worktree.
- **Proof of the change itself:**
  - A fix comes with a test that fails before it and passes after; say
    that you watched it fail.
  - Anything a person sees comes with screenshots of the real app against
    a dev stack: web at 1280 and 390 px, iOS from the simulator, light and
    dark where it matters. Nothing staged or mocked.
  - When only the real platform can tell, run a hosted check against
    preview (`cargo xtask e2e --hosted`, preview account only).
- **Gaps:** what you didn't verify and what could still go wrong. Never
  claim more than you checked.

Then get a Codex review of the diff, one round. Only a bug a person would
hit blocks: wrong money, lost or wrong data, a crash, a broken or slowed
call, or a security hole. Fix those, and file anything else in Linear. A
second round only checks the fixes, and there is no third.

Keep PRs small and many. Ten small proven PRs land faster and safer than
one large one.

## How work lands

- Paul reads the evidence and says merge; the coordinating session merges
  directly. There is no merge queue, required check or required approval.
- `master` runs one CI at a time (`check`, the e2e, and the Docker lanes
  when images change). Only the newest push waits, so a burst of merges
  is tested together at the tip. Green deploys preview.
- If `master` goes red, preview is not deployed. Fix it forward at once
  with another small PR, proven the same way.
- Paul tries preview, then promotes it to production by pushing
  `production`. That run keeps its own proof before it deploys.

## Rules and approvals

- Prefer an idiomatic Cloudflare feature, standard crate or maintained
  library when it simplifies the stack. Bespoke code needs its reason
  beside it (comment, decision or debt entry). A build step is welcome
  when it buys significant simplification or safety.
- Each runtime offers what it supports: the UI shows only what an agent's
  runtime can do (`cell/shell/agent-runtimes.js`), and we don't build
  parity across runtimes.
- One concern per PR. Work goes to github.com/finitecomputer/fragment
  as a branch and PR against `master`; names under `ci/` are refused.
  It merges on Paul's word, with its evidence (Proving your work).
- Only Paul updates `production`: his push or merge approves production
  deployment. Green master pushes deploy preview automatically. CI and
  rollout code define proof, preparation and rollback checks; preserve them.
- Production deploys by hand, other hosted fleet deploys, DNS changes,
  adding remotes, deleting Sprites, teardown and other irreversible
  actions need Paul's approval. Live Stripe setup is Paul's to run;
  setting remote secrets needs Paul or the coordinating session.
- Deployment configs are in private `finitecomputer/fragment-deploy`
  (`preview.jsonc`, `production.jsonc`). Commit config changes there;
  each environment's next deploy reads them. Never log their contents.
- Secrets live in Cloudflare Secrets Store, named in config and read by
  `cell/src/keys.rs`; see `docs/secrets.md` for exceptions and rotation.
  Never print secrets, pass values on command lines, or commit them.
  Deleting a secret is Paul's.
- Call Durable Objects only through `routed::send` / `send_again` so
  release mismatches are refused before acting. Stored rows and queued
  messages must stay readable across releases (`docs/MODEL.md`).
