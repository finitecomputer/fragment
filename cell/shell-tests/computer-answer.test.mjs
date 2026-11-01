// cell/shell/computer-answer.js's `answerTo` (Alex on #911): a card's answer
// for the computer is handed on only from a frame of the person's own
// fragment, at its own origin, for an agent of their computer; every other
// sender gets no port ticket, so delivers no answer.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { answerTo } from "../shell/computer-answer.js";

const origin = "https://talk--a1b2.finite.place";
const agents = new Map([["npub-juniper", { fragment: "juniper--k3x9" }]]);
const computer = { computer: "c1" };
const from = (o) => answerTo({ frame: "talk--a1b2", role: "owner", origin, sentFrom: origin, agent: "npub-juniper", agents, computer, ...o });

test("the person's own fragment, at its origin, for their own agent: that agent's computer", () => {
  assert.deepEqual(from({}), { agent: "juniper--k3x9" });
});

test("a fragment shared with them (an editor's, a viewer's): nothing, no ticket", () => {
  assert.equal(from({ role: "editor" }), null);
  assert.equal(from({ role: "viewer" }), null);
  assert.equal(from({ role: undefined }), null);
});

test("an unknown frame, or a page not at its fragment's origin: nothing, no ticket", () => {
  assert.equal(from({ frame: undefined, role: undefined }), null);
  assert.equal(from({ sentFrom: "https://elsewhere.example" }), null);
  assert.equal(from({ origin: null }), null);
});

test("their own fragment, for an agent not theirs or with no computer: refused in words, no ticket", () => {
  assert.deepEqual(from({ agent: "npub-someone-else" }), { refused: "Only its owner answers this, from their own Finite." });
  assert.deepEqual(from({ computer: null }), { refused: "Only its owner answers this, from their own Finite." });
});
