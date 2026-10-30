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
- Contracts: `docs/api.md` (wire), `docs/computers.md` (image),
  `docs/bridge.md` (bridge), `docs/chat-records.md` (chat records),
  `docs/durable-computers.md` (durability, including outstanding work),
  `docs/secrets.md` (secret custody), `docs/ledger.md` (usage),
  `docs/billing.md` (orgs and Stripe), `docs/hermes-relay.md` (runtime).
- `docs/published-fragments.md`: primitives that must stay expressible.
- `docs/technical-debt-ledger.md`: the only place shortcuts may live;
  include the condition for removing each one.
- `docs/finite-integration.md`: integration concerns and their current
  stand-ins; update the relevant row when changing one.
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

## Rules and approvals

- Prefer an idiomatic Cloudflare feature, standard crate or maintained
  library when it simplifies the stack. Bespoke code needs its reason
  beside it (comment, decision or debt entry). A build step is welcome
  when it buys significant simplification or safety.
- One concern per PR. Work goes to github.com/finitecomputer/fragment
  as a branch and PR against `master`; names under `ci/` are refused.
  The coordinating session reviews PRs once CI is green.
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
