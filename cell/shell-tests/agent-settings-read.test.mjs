// cell/shell/agent-settings.js `readWithin` and `cardsFor` (FIN-424): an
// agent's card reads its settings (agent.json) within a deadline, drawn at
// once and saying it is reading, so one slow read holds up nothing else;
// a roster that changes keeps the cards of the agents still there; and the
// Edit dialog of an agent removed (in another tab) is closed, its late
// answers dropped (`editTenancy`). Its
// deadline here is the test's own (`after`), and each read a promise the
// test answers: no time passes.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { cardsFor, editTenancy, readWithin, SETTINGS_READ_MS } from "../shell/agent-settings.js";

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

// The one Edit dialog and its slot, as the shell makes them: closing fires
// its `onclose` (a dialog's close event), and what it holds is the slot's.
function editing() {
  const d = {
    dialog: { open: false, onclose: null, showModal() { this.open = true; }, close() { this.open = false; this.onclose?.(); } },
    slot: { held: [], replaceChildren(...nodes) { this.held = nodes; } },
  };
  return { d, tenancy: editTenancy(() => d) };
}
const answer = () => {
  const out = {};
  out.promise = new Promise((resolve, reject) => Object.assign(out, { resolve, reject }));
  return out;
};

test("the agent being edited removed, its save on its way: its Edit closes and empties, its fields are not given back, and the save's late success says nothing", async () => {
  const { d, tenancy } = editing();
  const wren = { card: "wren--c3d4" };
  let givenBack = 0;
  d.slot.replaceChildren("wren's fields");
  tenancy.lend(wren, () => givenBack++);
  d.dialog.showModal();
  const save = answer();
  const saved = tenancy.whileLive(wren, save.promise);
  // another tab removes Wren; this one's Settings draw their Agents again
  tenancy.retire(wren);
  assert.equal(d.dialog.open, false);
  assert.deepEqual(d.slot.held, [], "none of Wren's fields left in it");
  assert.equal(givenBack, 0, "its fields go back to no card");
  save.resolve({ ok: true });
  assert.equal(await settled(saved), "pending", "its success never reaches its picks");
});

test("a read of the removed agent's card that answers late, or fails late, changes nothing", async () => {
  const { tenancy } = editing();
  const wren = { card: "wren--c3d4" };
  const read = answer(), failed = answer();
  const shown = tenancy.whileLive(wren, read.promise), said = tenancy.whileLive(wren, failed.promise);
  tenancy.retire(wren);
  read.resolve({ tier: "medium" });
  failed.reject(new Error("refused"));
  assert.equal(await settled(shown), "pending");
  assert.equal(await settled(said.catch(() => "failed")), "pending");
});

test("another agent removed while one is edited: its Edit stays open, its save lands, and closed, its fields go back", async () => {
  const { d, tenancy } = editing();
  const rosalind = { card: "rosalind--a1b2" }, wren = { card: "wren--c3d4" };
  let givenBack = 0;
  d.slot.replaceChildren("rosalind's fields");
  tenancy.lend(rosalind, () => givenBack++);
  d.dialog.showModal();
  const save = answer();
  const saved = tenancy.whileLive(rosalind, save.promise);
  tenancy.retire(wren);
  assert.equal(d.dialog.open, true);
  assert.deepEqual(d.slot.held, ["rosalind's fields"]);
  save.resolve({ tier: "medium" });
  assert.deepEqual(await saved, { tier: "medium" });
  d.dialog.close();
  assert.equal(givenBack, 1);
});
