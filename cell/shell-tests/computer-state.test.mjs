// cell/shell/computer.js's `stateSaid`: Settings' State.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { stateSaid } from "../shell/computer.js";

// Settings' State (Paul on #1053): a restart's save says Saving; a sleep
// that is no restart is going to sleep; every other phase as it is.
test("State: Saving while a restart saves, Going to sleep for any other sleep", () => {
  assert.equal(stateSaid({ phase: "sleeping" }, true), "Saving");
  assert.equal(stateSaid({ phase: "sleeping" }, false), "Going to sleep");
  assert.equal(stateSaid({ phase: "starting" }, true), "Starting");
  assert.equal(stateSaid({ phase: "awake" }, false), "Awake");
  assert.equal(stateSaid({ phase: "wont_wake" }, true), "Won't start");
});
