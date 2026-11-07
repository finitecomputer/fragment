// cell/shell/agent-settings.js `readRoster`: the agents Settings draw are
// a snapshot of the roster, each drawn once its settings are read (FIN-424:
// a card drawn first grew as its read answered, moving New agent from under
// a hand on its way to it). Every agent of the snapshot has an answered
// read, whatever the roster does during the wait; none other is read.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { readRoster } from "../shell/agent-settings.js";

// Robin's agents, each agent.json read held until the test answers it
function world(names) {
  const live = names.map((fragment) => ({ fragment }));
  const held = new Map();
  const asked = [];
  const read = (fragment) => {
    asked.push(fragment);
    return new Promise((resolve) => held.set(fragment, resolve));
  };
  return { live, held, asked, read };
}
const settled = async (p) => {
  let done = false;
  p.then(() => { done = true; });
  for (let i = 0; i < 5; i++) await null;
  return done;
};

test("an agent come during the wait is not drawn this load, and every agent drawn has its read answered", async () => {
  const w = world(["wren--a1b2"]);
  const started = new Map(w.live.map((a) => [a.fragment, w.read(a.fragment)]));
  const mine = [...w.live];
  const reads = readRoster(mine, started, w.read);
  // the list finds another agent while Settings wait for Wren's read
  w.live.push({ fragment: "quill--c3d4" });
  assert.equal(await settled(reads), false, "it waits for Wren's read");
  w.held.get("wren--a1b2")({ tier: "medium" });
  const got = await reads;
  assert.deepEqual([...got.keys()], ["wren--a1b2"]);
  for (const a of mine) assert.deepEqual(await got.get(a.fragment), { tier: "medium" });
  assert.deepEqual(w.asked, ["wren--a1b2"], "Quill is read by the load that draws it");
});

test("an agent come before the snapshot but after the reads began is read now; one gone is not drawn", async () => {
  const w = world(["wren--a1b2", "rook--e5f6"]);
  const started = new Map(w.live.map((a) => [a.fragment, w.read(a.fragment)]));
  // before the snapshot: Rook removed, Quill made
  w.live.splice(1, 1, { fragment: "quill--c3d4" });
  const reads = readRoster([...w.live], started, w.read);
  w.held.get("wren--a1b2")({});
  assert.equal(await settled(reads), false, "it waits for Quill's read too");
  w.held.get("quill--c3d4")(null);
  const got = await reads;
  assert.deepEqual([...got.keys()], ["wren--a1b2", "quill--c3d4"]);
  assert.equal(await got.get("quill--c3d4"), null, "an unread one is drawn saying so");
  assert.deepEqual(w.asked, ["wren--a1b2", "rook--e5f6", "quill--c3d4"]);
});

test("no agents: nothing read, nothing waited for", async () => {
  const w = world([]);
  const got = await readRoster([], new Map(), w.read);
  assert.equal(got.size, 0);
  assert.deepEqual(w.asked, []);
});
