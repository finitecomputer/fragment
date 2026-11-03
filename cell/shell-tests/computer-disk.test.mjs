// cell/shell/computer.js's `diskSaid`: what the person is told of a disk
// filling (the `disk_filling` notice), in plain words: the room left, and
// what to do before it runs out.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { DISK_TITLE, diskSaid } from "../shell/computer.js";

test("a disk filling: the room left, in GB, and what to do", () => {
  assert.equal(DISK_TITLE, "Your computer is almost full");
  assert.equal(
    diskSaid({ kind: "disk_filling", total: 20_000_002_048, free: 3_240_662_016 }),
    "It has 3.2 GB of space left. When it's full, your agents can't save their work. Delete files you don't need, or ask an agent to tidy up.",
  );
});

test("under a GB, in MB", () => {
  assert.match(diskSaid({ kind: "disk_filling", total: 20_000_002_048, free: 420_000_000 }), /^It has 420 MB of space left\./);
});
