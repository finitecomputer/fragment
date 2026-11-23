# Dependency scanning

Every third-party dependency the repo locks is checked against public
advisory databases. This runs on every change to master, every day, and on
any pull request that changes a dependency. A known vulnerability fails the
scan unless it is on a reviewed ignore list, and each entry on that list
gives its reason and the Linear issue that will remove it. This page covers
what is scanned, when, how a finding is handled, and what is not scanned
yet.

The command is `cargo xtask audit` (`xtask/src/audit.rs`). CI runs it in
its own workflow, `.github/workflows/audit.yml`. It was added for FIN-576.

## What is scanned

| Lockfile | What it builds | Tool and database |
|---|---|---|
| `Cargo.lock` | the CLI, the shared core and protocol crates, the identity door, dev and test tooling | cargo-deny, RustSec |
| `cell/Cargo.lock` | the production Cloudflare Worker (the cell) | cargo-deny, RustSec |
| `images/Cargo.lock` | what runs on computers: the bridge, the stub, Hermes' boot, the SimpleX door | cargo-deny, RustSec |
| `bench/search/Cargo.lock` | the search index benchmark (never shipped) | cargo-deny, RustSec |
| `package-lock.json` | the JavaScript the platform builds and deploys with: wrangler, esbuild, and the Cloudflare Sandbox SDK that the cell bundles | npm audit, GitHub Advisory Database |
| `templates/slides/package-lock.json` | the slides template's build | npm audit, GitHub Advisory Database |

A unit test (`every_tracked_lock_is_audited`) runs on every pull request
as part of `cargo xtask check`. It fails if git tracks a `Cargo.lock` or
`package-lock.json` that the scan does not cover, so a new workspace or
package cannot be added without being scanned.

### Rust: cargo-deny

[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) runs its
`advisories` check against the [RustSec advisory database](https://rustsec.org).
It fetches the database fresh on every run. Each workspace has its own
policy file, `deny.toml`, next to its `Cargo.toml`:

- **Vulnerabilities and unsound code** (advisories rated `unsound`) fail
  the scan at any depth of the dependency graph, with all features on.
  This includes dev and build dependencies.
- **Unmaintained crates** fail the scan when a workspace crate depends on
  them directly (`unmaintained = "workspace"`). An unmaintained crate
  deeper in the graph is not a finding by itself: we can't replace it
  without replacing whatever depends on it. If it ever gets a
  vulnerability advisory, that fails the scan like any other.
- **Yanked versions** are reported as warnings.
- **Stale ignores fail the scan.** An ignore entry that no longer matches
  anything fails the run (`unused-ignored-advisory = "deny"`), so the list
  only ever holds live findings.

CI installs cargo-deny 0.20.2 with a checksum-verified release binary. The
scan refuses to run under any other version. A test checks that the
workflow and `xtask/src/audit.rs` name the same version.

### JavaScript: npm audit

`npm audit --package-lock-only` reads each lockfile and queries the
[GitHub Advisory Database](https://github.com/advisories). It installs
nothing and runs no package scripts. It runs on the repo's pinned Node
release and that release's own npm (`docs/pinned-node.md`), never a
developer's install. Development dependencies are included, because they
build and deploy what ships.

npm has no ignore file of its own, so the ignore list is `NPM_IGNORED` in
`xtask/src/audit.rs`. Each entry names the lockfile, the GHSA id and the
reason. Any advisory not on the list fails the scan, at any severity. So
does any entry that no longer matches.

## When it runs

`.github/workflows/audit.yml` runs:

- on every push to `master`, so the tree that deploys preview is scanned;
- every day at 06:17 UTC, so an advisory published against code that
  hasn't changed is caught within a day;
- on any pull request that changes a `Cargo.lock`, `Cargo.toml`,
  `package-lock.json`, `package.json`, `deny.toml`, `xtask/src/audit.rs`
  or the workflow itself;
- by hand (`workflow_dispatch`), and locally with `cargo xtask audit`.

The scan is not part of `cargo xtask check`, and no other job waits for
it. The scan's result can change with no change to our code: a new
advisory published upstream turns it red. If it gated `check`, every
unrelated pull request would turn red with it, and so would master's
deploy of preview. Keeping it in its own workflow means a new advisory
shows up as a red `audit` run on master and in the daily run, which we
then triage. It does not block unrelated work.

## How a finding is handled

A red `audit` run is triaged when it is seen, by whoever is on the change
or, for the daily run, the security owner (FIN-570). The patch policy
(FIN-592) sets the deadlines. Each finding gets one of three outcomes:

1. **Fix it.** If a compatible release fixes it (a lockfile-only update
   such as `cargo update -p <crate> --precise <version>`), ship that as
   its own small pull request with the usual evidence. The ignore entry,
   if there was one, leaves in the same change: a stale entry fails the
   scan.
2. **Track it.** If the fix needs real work (a major-version migration, or
   a bump that needs the full e2e), add an ignore entry and file a Linear
   issue in the "Google CASA security assessment" project, labelled
   Security. The entry goes in the workspace's `deny.toml` (Rust) or in
   `NPM_IGNORED` (npm). It must say where the dependency is used, why the
   exposure is acceptable for now, and which issue removes it. A reviewer
   checks the reason in the pull request.
3. **Accept it.** If the dependency never ships (test fakes, a benchmark)
   or the advisory's conditions can't occur in our use, add an ignore
   entry whose reason says why. The entry stays only while the advisory
   matches. When upstream fixes it and we update, the scan fails until
   the entry is removed.

An ignore entry is never added without a reason. The policy tests in
`xtask/src/audit.rs` reject an entry with an empty reason.

## Findings at introduction (23 November 2026)

Lockfile-only patch updates fixed these: rustls 0.23.46, rustls-webpki
0.103.17, webpki-roots 1.0.12, tokio 1.53.3, url 2.5.9, and zstd 0.13.5 /
zstd-safe 7.4.1 (benchmark only). Together they close RUSTSEC-2026-0285,
-0346, -0348, -0349, -0381, -0382, -0386, -0391, -0398, -0403 and -0592.

These stay on the ignore lists:

| Advisory | Where | Handling |
|---|---|---|
| RUSTSEC-2026-0387, crypto-bigint 0.5 timing | k256/p256 0.13 in every workspace. The cell signs Web Push and code.storage tokens with P-256, and the CLI signs NIP-98 with k256. | Tracked: FIN-636 (move to the 0.14 line) |
| RUSTSEC-2026-0459, bundled SQLite before 3.54 | rusqlite 0.40: the CLI, dev tooling, and on computers the SimpleX door and Hermes' boot. Only a build dependency in the cell, not part of its Wasm. | Tracked: FIN-637 (rusqlite 0.41) |
| RUSTSEC-2023-0071, rsa (Marvin) | `crates/fakes` only, the vendor fakes used in tests | Accepted: nothing that ships links rsa |
| GHSA-mrhx-c7xc-m3w5, zod before 4.6.6 | `@cloudflare/sandbox` 1.0.0, bundled into the cell | Tracked: FIN-639 (sandbox 1.0.3) |
| Eight undici advisories and GHSA-wq5f-xc86-pv6w (sharp) | miniflare under wrangler 4.145, used for dev, e2e and deploy. Not in the Worker. | Tracked: FIN-640 (wrangler 4.167) |
| GHSA-g3hv-75wg-m249, minimatch | the slides template's build, through shadcn | Accepted: no fixed release yet, and the build uses only its own glob patterns |

## Not scanned yet

These are tracked in FIN-638:

- the Python packages installed into the Hermes image
  (`images/hermes/*-requirements.txt`), and the Hermes base image's own
  Python environment and OS packages;
- the iOS app's Swift packages (`Package.resolved`);
- the `playwright` npm package that the Hermes image installs globally.

Container images are built and pinned by digest (`docs/computers.md`), but
nothing scans them for vulnerabilities yet.
