# Solid 2 shell spike

2026-10-28. A working Apps slice in Solid 2 and strict TypeScript, against
the local Fragment API. This is an experiment, not a replacement shell.

## Findings

Porting the UI is straightforward. `For` keys keep each row and its local
state stable; async memos with `Loading`/`Errored` own loading and failures;
`onCleanup` owns timers and object URLs. Component props and API types
replace the shell's broad helper object. The compiler caught invalid ARIA
attribute types and a v1 error-boundary import during the port.

The expensive work in a complete shell port is preserving behavior:
first-run/resume, per-origin frames and consent, drafts, browser history,
ordered writes, and safe reload during deploys. Solid does not replace
those contracts. The existing browser suite is the acceptance test. This
spike establishes feasibility for the presentation layer, not a credible
estimate for replacing all those flows.

### Code size

Same Vite build, target ES2022, default minifier; JS only, transitive chunks
deduplicated, gzip per file using Node's `gzipSync`. The shared shell CSS,
fonts and images are excluded from all three. See `results/size.json`.

| Entry | Minified JS | Gzip JS |
|---|---:|---:|
| Current vanilla Apps + icon/activity dependencies | 21,255 B | 7,930 B |
| Solid partial Apps + small navigation scaffold | 50,909 B | 20,043 B |
| Entire current vanilla shell | 161,686 B | 55,820 B |

Solid adds about **12 KB gzip** to this Apps slice even with incomplete
feature coverage. The runtime is a shared cost, so a complete shell might
amortize it; these numbers do not establish savings for a full rewrite.
The full vanilla bundle is a size reference, not an equivalent competitor
to the partial Solid app. Bundling/minifying the current modules is an
independent improvement worth measuring before choosing a framework.

### Rendering

A Chromium microbenchmark uses the real current `createAppsPage` and the
real Solid `Apps` component, the same shell stylesheet and 1280×900
viewport, and 100 read-only app rows. It changes one Dock flag each time,
forces layout, warms up 20 updates and measures the next 50. Network
responses are controlled and immediately resolved; fonts finish loading
before timing. Both final DOMs are checked for the expected 30 docked rows.
See `results/render.json`.

| One row update among 100 | Median | p95 |
|---|---:|---:|
| Current vanilla Apps | 27.9 ms | 30.4 ms |
| Solid Apps | below 0.1 ms clock resolution | 0.1 ms |

The current implementation constructs candidate DOM for every row and
compares serialized signatures. Solid updates the changed keyed row.
This is useful evidence about list rendering; it measures neither startup
nor server latency, and feature/markup coverage differs. It is not a
full-shell speedup claim. An incremental vanilla row update is another
way to address this cost.

## Scope

Implemented against real endpoints: list/watch, inline rename, Dock,
icon generation/upload/pick, optional pretty-address claim, sharing,
preview refresh with elapsed progress, and opening an app. Writes await
the server and then reload the authoritative list.

Missing: initial preview/cache and all archive/unarchive behavior, full
access/member summaries, icon removal, the shell's full icon vocabulary,
chat rendering/framing, agent settings, connections, onboarding, billing,
release/session lifecycle, and most navigation. Open uses a new tab to
the real platform's authorization route; the experiment's different
origin does not qualify as a trusted shell frame. No fixture data is
used in the normal app. `benchmark.html` alone is synthetic.

The prototype loaded the three real local sample apps, including the
claimed private address, without script exceptions or desktop overflow.
The build passes strict TypeScript checking. The package is isolated;
there is no production dependency or worker routing change. The repo's
source inventory explicitly classifies this directory as a local
experiment in `xtask/src/eval_sinks.rs`.

## Run locally

Use xtask's pinned Node (currently v24.22.1) and its own npm, with that
Node's bin directory first on PATH. Do not use the system Node.

1. Run the Apps branch's `cargo xtask dev` at port 8790 and sign in there.
2. From this directory, use the pinned npm to run `ci --ignore-scripts`.
3. Set `FRAGMENT_BASELINE_ROOT` to the Apps checkout, then run `npm run
   build` with the pinned npm and `npm run dev` to serve port 5180.
4. Open <http://127.0.0.1:5180/>. Cookies on 127.0.0.1 are shared across
   ports. The dev-only proxy sets the configured local origin on API
   requests; it is never part of a deploy.

The baseline defaults to this worktree's repository root. Set the variable
when comparing with a separate Apps worktree. Vite's `dist/.vite/manifest.json`
records each entry and its dependent chunks. Sum only the reachable `.js`
files, once each; gzip each file independently. `benchmark.html` exposes
`window.solidBenchmark.update(rows)`, which flushes Solid's updates; the
benchmark harness can drive it without changing product code.

Exact dependency versions are locked. Solid is **2.0.0-rc.19**, not a
stable 2.0 release ([official releases](https://github.com/solidjs/solid/releases));
the Vite plugin uses the new native compiler. V2's component and async
APIs differ from v1 ([official discussion](https://github.com/solidjs/solid/discussions/2995)).

## Suggested next step

Continue feature-module extractions now. If the component model is worth
the build/runtime cost, port one isolated, parity-complete section behind
the existing shell's composition boundary and run its existing browser
checks. Keep trusted frame handling and release/session behavior in their
tested modules until deliberately ported. A whole-shell rewrite should
follow evidence from that parity slice, not these partial bundle numbers.
