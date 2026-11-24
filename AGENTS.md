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
- Engineering style: `docs/engineering-style.md`: hard cuts, limits,
  release assertions, typed errors, and valid/invalid/replay/restart tests.
  Tooling is Rust.

## How work is asked for

A Linear ticket is a problem statement: what hurts, for whom, and why it
matters. It is not a plan or a list of PRs. Plans, research and notes go
in its comments or outside the repo, never in a commit.

Any ticket can be built, but Paul merges no PR until he has commented on
its root problem statement. His comment protects The Fragment Way, often
as "yes, solve this, but don't do X, and line it up with One Right Way".
He looks for:

- pieces built one at a time that should be one abstraction;
- a regression in quality;
- work that is sloppy or not ready;
- complication nobody needed, and gold plating.

The patterns are emergent, named as Paul finds them. So far:

- **One Right Way:** one permission decision for every caller
  (`fragment_core::permit`).
- **One Pipe:** every paid call goes through one gateway and is billed
  there, with no hooks scattered elsewhere.
- **Untangle:** a typical change lands in one place, and state is derived
  from its one authority, never a copy kept in sync.

Say in the PR which pattern your change follows, or that it follows none.
In Paul's words:

- "find a simple understandable core model from which the right things
  emerge. don't let codex trick you into making 10,000 little edge case
  edits" (2026-11-23)
- "if two PRs are meaningless without each other, why aren't they one PR?
  … avoid xeno's paradoxes!" (2026-11-24)

If a rule here fights the task in front of you, say so plainly, and get
Paul's sign-off before you break it.

## Where things live

- `cell/`: Rust Worker and JS shims; `cell/shell/`: platform pages. The
  same crate built with `--features admin` is the operators' admin
  Worker (`cell/src/admin.rs`, `cell/wrangler.admin.jsonc`).
- `cli/`: CLI; `xtask/`: build, check, dev, e2e, deployment tooling.
- `crates/proto`: wire types; `crates/core`: pure logic and state machines;
  `crates/nip98`: authentication; `crates/templates`: release assets;
  `crates/fakes`, `crates/devstack`, `crates/e2e`: local vendor fakes and harness.
- `crates/identity-door`: a self-hosted box's sign-in, an OpenID Connect
  relying party behind the WorkOS endpoints the cell calls
  (docs/self-host.md, seam 4).
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
- `cargo xtask skills-catalog`: write the index of Hermes'
  optional skills (`cell/shell/optional-skills-hermes.json`) from the pinned
  Hermes image, through Docker. Run it when the pin moves; a host test
  fails while the catalog names another image (its tag or its digest).
- `cargo xtask dev [--clean] [--real-agent [--models <config>]]`: dev stack
  at :8790, fragments at `http://<name>.fragment.localhost:8790/`.
  Default vendors are fakes. `--real-agent` calls real text models using
  `FRAGMENT_DEV_MODEL_KEY_FILE`. CLI: set `FRAGMENT_HOST=http://127.0.0.1:8790`
  and run `fragment login`.
- `--runtime celld` on `cargo xtask dev`, and `FRAGMENT_E2E_RUNTIME=celld`
  on the e2e (both with `CELLD_BIN`): the self-hosted lane's runtime,
  no computers there (`docs/self-host.md`).
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

The PR body follows `.github/pull_request_template.md`. A bot closes a
PR whose body doesn't; fix the body and reopen it. #1540 is a good
example. Under "Why this design", and in your report, put an evidence
pack:

- **What and why:** what changed and why, in plain words. Say whether it
  touches the wire, stored rows or money.
- **What ran:** each command with its result and counts, on the final head
  merged with `master` as it was when the proof started; name both SHAs.
  `master` moving while a proof runs doesn't restart it:
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
    dark where it matters. Nothing staged or mocked. Attach them to the PR
    with the gh CLI, which uploads images and videos; a screenshot that
    stays on your machine isn't evidence, and none goes in the repo:

    ```sh
    gh pr create --title "…" --body "…" --attach './after.png#The new sheet'
    gh pr edit 123 --attach ./after-390-dark.png     # appended to the body
    gh pr comment 123 --body "Before and after:" \
      --attach ./before.png --attach ./after.png
    ```
  - When only the real platform can tell, run a hosted check against
    preview (`cargo xtask e2e --hosted`, preview account only).
- **Gaps:** what you didn't verify and what could still go wrong. Never
  claim more than you checked.

A Codex review of the diff then runs by itself when the PR opens and posts
its verdict on the PR: don't start one, read it. One round. Only a bug a
person would hit blocks: wrong money, lost or wrong data, a crash, a broken
or slowed call, or a security hole. Fix those, and file anything else in Linear. A
second round only checks the fixes, and there is no third.

Keep PRs small, and each one whole: one thing that stands on its own,
something a person can use or a step that is safe and useful by itself.
If two PRs mean nothing without each other, they are one PR. Split when
a piece stands alone, or must wait (a deletion after a migration has
run). A plan that grows a PR each review round is a smell; merge the
pieces back.

### Hit every surface

The most common gap is a change that works where it was tested and is
missing everywhere else. Before you call it done, say which of these
applied:

- **Clients:** the web shell (`cell/shell/`), the iOS app and the CLI.
- **Runtimes:** Hermes and Claude. Each offers only what it supports
  (`cell/shell/agent-runtimes.js`), so decide for each one, even when the
  answer is "not here".
- **The agent:** can an agent do it too (a skill, the CLI), or only a
  person?
- **The wire:** `crates/proto`, then `cargo xtask wire`. Older clients
  must still read every answer.
- **Self-host:** celld looks like Cloudflare to the cell; there are no
  runtime shims in `cell/`.
- **Reverse states:** every way in needs a way out and a way to see it:
  archive and unarchive, link and unlink, delete. A one-way door is a bug.

### Ways to hurt yourself

- **Killing by pattern.** `pkill -f` and `pgrep` match your own shell,
  whose command holds the pattern, and other sessions' stacks. Kill only a
  PID you started, or a dev stack by its own process group.
- **The shared dev port.** :8790 is Paul's `cargo xtask dev`. Run yours on
  another port.
- **Printing what is secret.** Never print a secret, a deploy config or
  the CLI's config (it holds a key), not even to check it.
- **Sweeping commits.** Never run `git add -A`, since a dev stack's files
  sit beside yours, or `cargo fmt`, which reformats all of `cell/`.

## How work lands

- Paul reads the evidence and says merge; the coordinating session merges
  directly. There is no merge queue, required check or required approval.
- `master` runs one CI at a time (`check` and the e2e). Only the newest
  push waits, so a burst of merges is tested together at the tip. Green
  deploys preview.
- If that run goes red, preview is not deployed. Fix it forward at once
  with another small PR, proven the same way.
- When `images/` changes, the Docker and real-Hermes lanes run in their
  own workflow (`images.yml`) and don't gate the preview deploy. Check
  their result before counting on a new image.
- Paul tries preview, then promotes it to production by pushing
  `production`. That run keeps its own proof before it deploys.

## Rules and approvals

- When a documented decision changes, rewrite its text; don't append
  another account of the new behavior. Most code changes need no doc
  change: agents can read the code.
- Prefer an idiomatic Cloudflare feature, standard crate or maintained
  library when it simplifies the stack. Bespoke code needs its reason
  beside it (comment, decision or debt entry). A build step is welcome
  when it buys significant simplification or safety.
- Shell, templates and platform pages load no script, stylesheet or font
  from another domain (a security hole and a privacy leak): serve it
  ourselves in the release's files, async if it helps. `cargo xtask
  check` enforces it (`xtask/src/remote_loads.rs`).
- Each runtime offers what it supports: the UI shows only what an agent's
  runtime can do (`cell/shell/agent-runtimes.js`), and we don't build
  parity across runtimes.
- One concern per PR. Work goes to github.com/finitecomputer/fragment
  as a branch and PR against `master`; names under `ci/` are refused.
  It merges on Paul's word, with its evidence (Proving your work).
- Only Paul updates `production`: his push or merge approves production
  deployment. Master's run is the gate: a green master push deploys
  preview, a red one deploys nothing and is fixed forward. A PR's CI is
  `check` alone, required by nothing, with no merge queue: validate on
  this machine and show the evidence. CI and rollout code define proof,
  preparation and rollback checks; preserve them.
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
