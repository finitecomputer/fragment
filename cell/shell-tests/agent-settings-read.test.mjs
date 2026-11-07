// cell/shell/agent-settings.js `readWithin` and `cardsFor` (FIN-424): an
// agent's card reads its settings (agent.json) within a deadline, drawn at
// once and saying it is reading, so one slow read holds up nothing else;
// and a roster that changes keeps the cards of the agents still there. Its
// deadline here is the test's own (`after`), and each read a promise the
// test answers: no time passes.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { cardsFor, readWithin, SETTINGS_READ_MS } from "../shell/agent-settings.js";

// a read the test answers, and a deadline the test fires
function held() {
  const read = {};
  const fn = (signal) => {
    read.signal = signal;
    return new Promise((resolve, reject) => Object.assign(read, { resolve, reject }));
  };
  const deadline = { armed: null, disarmed: false };
  const after = (ms, fire) => {
    deadline.armed = ms;
    deadline.fire = fire;
    return () => { deadline.disarmed = true; };
  };
  return { read, fn, deadline, after };
}
const settled = async (p) => {
  let out = "pending";
  p.then((v) => { out = { value: v }; });
  for (let i = 0; i < 5; i++) await null;
  return out;
};

test("a read that answers in time is its answer, and its deadline is disarmed", async () => {
  const h = held();
  const got = readWithin(h.fn, SETTINGS_READ_MS, h.after);
  assert.equal(h.deadline.armed, SETTINGS_READ_MS);
  assert.equal(await settled(got), "pending", "it waits for its read");
  h.read.resolve({ tier: "medium" });
  assert.deepEqual(await got, { tier: "medium" });
  assert.equal(h.deadline.disarmed, true);
  assert.equal(h.read.signal.aborted, false);
});

test("a read still out at its deadline is unread (null), its request aborted; its late answer changes nothing", async () => {
  const h = held();
  const got = readWithin(h.fn, SETTINGS_READ_MS, h.after);
  h.deadline.fire();
  assert.equal(await got, null);
  assert.equal(h.read.signal.aborted, true);
  h.read.resolve({ tier: "medium" });
  assert.deepEqual(await settled(got), { value: null });
});

test("a read that fails is unread (null), its deadline disarmed", async () => {
  const h = held();
  const got = readWithin(h.fn, SETTINGS_READ_MS, h.after);
  h.read.reject(Object.assign(new Error("refused"), { status: 500 }));
  assert.equal(await got, null);
  assert.equal(h.deadline.disarmed, true);
});

// `cardsFor`: Settings' Agents drawn again as the roster changes keeps the
// card of each agent still there, the same one (its reads, its Edit open,
// a save on its way with it), makes one for an agent come, and drops one gone.

test("a roster changed while a card is edited: the agents still there keep their very cards, one come gets one, one gone has none", () => {
  const made = [];
  const make = (a) => {
    made.push(a.fragment);
    return { card: a.fragment, edits: [] };
  };
  const first = cardsFor([{ fragment: "rosalind--a1b2" }, { fragment: "wren--c3d4" }], new Map(), make);
  const editing = first.get("rosalind--a1b2");
  editing.edits.push("tier: medium (saving)");
  // another tab adds Quill: Rosalind's card is the one being edited still
  const added = cardsFor([{ fragment: "rosalind--a1b2" }, { fragment: "wren--c3d4" }, { fragment: "quill--e5f6" }], first, make);
  assert.equal(added.get("rosalind--a1b2"), editing);
  assert.equal(added.get("wren--c3d4"), first.get("wren--c3d4"));
  assert.deepEqual([...added.keys()], ["rosalind--a1b2", "wren--c3d4", "quill--e5f6"]);
  // and removes Wren: Rosalind's card, its save with it, is kept; Wren's goes
  const removed = cardsFor([{ fragment: "rosalind--a1b2" }, { fragment: "quill--e5f6" }], added, make);
  assert.equal(removed.get("rosalind--a1b2"), editing);
  assert.deepEqual(removed.get("rosalind--a1b2").edits, ["tier: medium (saving)"]);
  assert.equal(removed.get("quill--e5f6"), added.get("quill--e5f6"));
  assert.equal(removed.has("wren--c3d4"), false);
  assert.deepEqual(made, ["rosalind--a1b2", "wren--c3d4", "quill--e5f6"], "a card is made once, for an agent come");
});
