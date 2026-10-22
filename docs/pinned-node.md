# The pinned Node

The JavaScript the platform needs (wrangler, and the Sandbox SDK it
bundles; `package.json`) runs on one Node release, pinned in
`crates/devstack/src/node_release.rs` with the SHA-256 of each
platform's official tarball. `dev`, `e2e`, `deploy` and `teardown`
fetch it into `target/tools/` on first use (refusing a tarball whose
hash differs), run its own npm's `npm ci` when node_modules is missing
or stale, and start every JavaScript process on it, first on PATH, with
its caches under `target/cache/` (Browser Rendering's Chrome included).
`check` fetches it too, and runs `node --check` on it (no `npm ci`).
A `node` on PATH is never used, and there is no other: a machine the
pin has no tarball for runs nothing.

To move the pin:

1. Pick the release from https://nodejs.org/dist/index.json: the newest
   of an LTS line that wrangler's `engines` allows.
2. Fetch `https://nodejs.org/dist/v<version>/SHASUMS256.txt` and its
   `SHASUMS256.txt.sig`, and check the signature against the releaser's
   key from nodejs/node's README (`gpg --verify SHASUMS256.txt.sig
   SHASUMS256.txt`).
3. Set `NODE_VERSION`, and each `TARBALLS` hash to its
   `node-v<version>-<platform>.tar.gz` line (darwin-arm64, darwin-x64,
   linux-arm64, linux-x64).
4. `cargo xtask check`, then `cargo xtask e2e`: the first run fetches
   the new Node and runs `npm ci` with it. CI's cache of `target/tools`
   keys on that file, so it fetches afresh too.

