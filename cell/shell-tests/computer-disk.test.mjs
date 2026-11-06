// cell/shell/computer.js's `diskSaid`: what the person is told of a disk
// filling (the `disk_filling` notice), in one line, Paul's words: the room
// left, as the real free space, and what to do. And `stateSaid`, Settings'
// State.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { diskSaid, stateSaid } from "../shell/computer.js";

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

// Settings' State (Paul on #1053): a restart's save says Saving; a sleep
// that is no restart is going to sleep; every other phase as it is.
test("State: Saving while a restart saves, Going to sleep for any other sleep", () => {
  assert.equal(stateSaid({ phase: "sleeping" }, true), "Saving");
  assert.equal(stateSaid({ phase: "sleeping" }, false), "Going to sleep");
  assert.equal(stateSaid({ phase: "starting" }, true), "Starting");
  assert.equal(stateSaid({ phase: "awake" }, false), "Awake");
  assert.equal(stateSaid({ phase: "wont_wake" }, true), "Won't start");
});
