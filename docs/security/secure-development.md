# How code ships (FIN-594)

How a change goes from a branch to production. Sources: AGENTS.md
("Proving your work", "How work lands", "Rules and approvals") and
`.github/workflows/`.

## In short

1. A change is a small pull request against `master` on
   github.com/finitecomputer/fragment, one concern each.
2. Its author proves it on their own machine and puts the evidence in
   the PR.
3. It gets one round of code review.
4. Paul reads the evidence and says merge.
5. Master's CI runs the full checks and end-to-end tests (or reuses an
   earlier green master run of the same tree); only a green run deploys
   to the shared preview.
6. Paul tries preview, then promotes it to production by pushing the
   `production` branch. That push is the approval; its run checks again
   before it deploys.

## 1. The pull request

- Work goes to a branch and a PR against `master`. Branch names under
  `ci/` are refused (AGENTS.md, "Rules and approvals").
- One concern per PR; small PRs are preferred.
- Shortcuts are allowed only in `docs/technical-debt-ledger.md`, each
  with the condition for removing it.
- Bespoke code needs its reason beside it (a comment, a decision or a
  debt entry).

## 2. Proof before merge

The author proves the change; CI on a PR is not the safety net
(AGENTS.md, "Proving your work"). The PR body carries an evidence pack:

- **What and why**, and whether it touches the wire, stored rows or
  money.
- **What ran**, on the final head merged with current `master`:
  - `cargo xtask check`: source guards, JavaScript syntax, the schema
    lock, the wire lock, host tests, and clippy on host and Wasm with
    warnings denied;
  - `cargo xtask e2e --only …` for every end-to-end section that covers
    a changed file;
  - for `images/`: `cargo test --workspace` and the Docker tests;
  - for the cell: the Wasm size delta against `master`.
- **Proof of the change:** a fix comes with a test that fails before and
  passes after; anything a person sees comes with screenshots of the
  real app; a hosted check against preview when only the real platform
  can tell.
- **Gaps:** what was not verified.

## 3. Review

One round of review of the diff by Codex (an AI reviewer). Only a bug a
person would hit blocks: wrong money, lost or wrong data, a crash, a
broken or slowed call, or a security hole. Those are fixed; anything
else is filed in Linear. A second round checks only the fixes.

## 4. Merge

Paul reads the evidence and says merge; the coordinating session merges.
There is no merge queue, required check or required approval in GitHub
(AGENTS.md, "How work lands").

GitHub rulesets on the repo are live settings, not in the tree. As
read on 2026-11-23 (to confirm before submission; auditor evidence
provided privately):

- **`master`:** changes only through a pull request (merge commits
  only), no force push, no deletion. Required approvals: zero. No
  required status checks.
- **`production`:** no force push, no deletion, and updates restricted
  to one bypass actor.

## 5. Master's CI and preview

`.github/workflows/ci.yml`:

- **On a pull request:** `check` alone (`cargo xtask check`), which
  nothing requires.
- **On a push to master:** `check` and every end-to-end shard (five, each
  on its own runner, against the cell under workerd with vendor fakes),
  unless an earlier green master run already tested the same tree
  (`proven` job), in which case the shards are skipped. Only one master
  run is under way at a time; a burst of merges is tested together at
  the tip.
- **Deploy** runs only on a push to `master` or `production`. On master
  it deploys the one shared preview (`--branch preview`) when `check`,
  the end-to-end tests (or a `proven` tree) and the preview image are
  green. It reads the deploy config from finitecomputer/fragment-deploy
  with a read-only token, prints the config's commit and never its
  contents, and runs `cargo xtask deploy`. The deploy is verified:
  every Durable Object class must answer the new release within its
  bound. If not, it rolls back only when there is an eligible release
  before, and the rollback can itself fail, leaving the run red and the
  deploy stuck. The admin Worker deploys after the cell; its failure
  leaves the new cell live (`xtask/src/rollout.rs`, `back`;
  `xtask/src/deploy.rs`).
- **Red master** deploys nothing. The `master-red` job names what failed,
  and the fix goes forward as another PR.
- **Schema safety:** `check` holds the schema lock against what the push
  moved from, so a tree that drops a migration step cannot deploy
  (docs/MODEL.md, "Schemas").
- **Images** (`images/`) have their own workflow (`images.yml`): clippy,
  tests, and the Docker and real-Hermes tests. It does not gate the
  preview deploy.

## 6. Production

- Only Paul updates `production`; his push or merge approves the
  production deploy (decision 3; AGENTS.md).
- The production run checks again: `check` always runs; the end-to-end
  shards are skipped only when a green master run already tested the
  same tree (`proven` job).
- Production's images are the digests pinned in production's deploy
  config (`xtask/src/deploy.rs`: production uses its config's pins).
  The intended path copies the digest preview proved into production's
  registry by hand (`image.yml`, `workflow_dispatch` with `from`), but
  the same workflow can also build a fresh production image, whose
  digest preview never ran; it warns when it does. Which one a pin names
  is a human step (docs/computers.md, "Preview and production").
- Each GitHub environment (`preview`, `production`) holds its own
  deploy secrets, set by Paul (docs/secrets.md, "Files").
- CLI releases are built from tags (`release.yml`).

## Platform rules enforced in code

- Calls between Durable Objects go only through `routed::send` and
  `send_again`, so a call from another release is refused before it acts
  (docs/MODEL.md, "Calls between releases"; held by
  `xtask/src/guards.rs`).
- Schema changes only as new numbered steps, never edited
  (`crates/core/migrations/`, `schema.lock`; held by `cargo xtask check`).
- Wire changes an older client would feel fail `check`
  (`crates/proto/wire.lock`).
- JavaScript tools come from `package.json` on xtask's pinned Node, never
  a developer's own install (docs/pinned-node.md).

## Gaps

- No required review or required check on `master` in GitHub: the gate
  is Paul's word and master's CI.
- Review is by an AI reviewer, one round; there is no required human
  code review beyond Paul reading the evidence.
- No security-specific step (SAST) is in the workflows; dependency
  scanning is dependency-scanning.md (in progress, FIN-576).
- The repo's GitHub security settings (secret scanning, push
  protection, Dependabot security updates) are to be turned on; see
  FIN-635. Auditor evidence of the current settings provided privately.
- Some third-party GitHub Actions are pinned by tag (`actions/checkout@v4`),
  others by commit SHA (`Swatinem/rust-cache`).

## To confirm

- Paul: whether to turn on GitHub secret scanning and push protection
  (free on a public repo; FIN-635).
- Paul: who holds write and admin on the GitHub repo and org (anyone
  with write can merge a PR to `master`, which deploys preview).
