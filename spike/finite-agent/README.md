# The Finite Agent spike (never merged)

pi-durable, pi's durable agent harness, as an agent fragment's own code:
the same shape as Cloudflare's PiHarness, on fragment primitives. Its
design and what it found are in Linear (Finite Agent phase 1, 1b, 2).

- `src/agent.js`: the agent. One live operation, `drive`: it follows a
  chat (`{follow}`), and takes each followed record as a turn in that
  chat's own pi conversation, writing the bridge's records
  (docs/chat-records.md).
- `fragment.json`, `app.mjs`: the fragment's manifest and code; the code is
  the bundle `build.mjs` makes (`applib/agent.mjs`).
- `package.json`: this folder's own pinned dependencies (pi-durable,
  pi-ai, chord, pi-schema, esbuild); nothing outside it uses them.

Build with the repo's pinned Node: `PATH=<repo>/target/tools/node-…/bin:$PATH
npm ci && node build.mjs ../../crates/e2e/fixtures/finite-agent`. The e2e
section `finite-agent` deploys that output into an agent fragment.
