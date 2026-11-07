// cell/shell/agent-settings.js `readWithin`: an agent's card reads its
// settings (agent.json) within a deadline, drawn at once and saying it is
// reading, so one slow read holds up nothing else (FIN-424). Its deadline
// here is the test's own (`after`), and each read a promise the test
// answers: no time passes.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { readWithin, SETTINGS_READ_MS } from "../shell/agent-settings.js";

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
