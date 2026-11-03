// cell/shell/computer.js's `diskSaid`: what the person is told of a disk
// filling (the `disk_filling` notice), in one line, Paul's words: the room
// left, as the real free space, and what to do.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { diskSaid } from "../shell/computer.js";

test("a disk filling: the room left, in GB, and what to do", () => {
  assert.equal(
    diskSaid({ kind: "disk_filling", total: 20_000_002_048, free: 3_240_662_016 }),
    "Your computer is almost full (3.2 GB left). Delete files you don't need, or ask an agent to tidy up.",
  );
});

test("under a GB, in MB", () => {
  assert.equal(
    diskSaid({ kind: "disk_filling", total: 20_000_002_048, free: 420_000_000 }),
    "Your computer is almost full (420 MB left). Delete files you don't need, or ask an agent to tidy up.",
  );
});
