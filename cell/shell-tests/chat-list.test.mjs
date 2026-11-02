// cell/shell/chat-list.js's sections (Paul, 2026-10-31): a section's
// order, its pinned chats first, then the rest, each the newest first; and
// what it shows, its first five (pinned ones among them), the open one
// wherever it is, each whose row says what its agent is doing (never folded
// under More), and all of them unfolded.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { pinnedFirst, rowStatus, sectionShown } from "../shell/chat-list.js";

const chat = (name, previewAt, pinned = false) => ({ name, previewAt, pinned });
const names = (list) => list.map((f) => f.name);

test("pinned chats come first, then the rest, each the newest first", () => {
  const list = [chat("old", 1), chat("pinned-old", 2, true), chat("new", 9), chat("pinned-new", 5, true), chat("made-now", undefined)];
  assert.deepEqual(names([...list].sort(pinnedFirst)), ["pinned-new", "pinned-old", "made-now", "new", "old"]);
});

test("a section shows its first five, pinned ones counted among them, and More past them", () => {
  const list = [1, 2, 3, 4, 5, 6, 7].map((n) => chat(`c${n}`, n * 10)).concat([chat("p1", 1, true), chat("p2", 2, true)]);
  const { shown, more } = sectionShown(list);
  assert.deepEqual(names(shown), ["p2", "p1", "c7", "c6", "c5"]);
  assert.equal(more, true);
});

test("the open chat shows wherever it is; unfolded, every chat in order", () => {
  const list = [1, 2, 3, 4, 5, 6, 7].map((n) => chat(`c${n}`, n * 10));
  assert.deepEqual(names(sectionShown(list, { current: "c1" }).shown), ["c7", "c6", "c5", "c4", "c3", "c1"]);
  assert.deepEqual(names(sectionShown(list, { unfolded: true }).shown), ["c7", "c6", "c5", "c4", "c3", "c2", "c1"]);
});

test("a chat its agent is waiting in or working in shows however far down it is, in its place, never under More", () => {
  const list = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => chat(`c${n}`, n * 10));
  assert.deepEqual(names(sectionShown(list, { busy: new Set(["c1", "c3"]) }).shown), ["c8", "c7", "c6", "c5", "c4", "c3", "c1"]);
  assert.equal(sectionShown(list, { busy: new Set(["c1"]) }).more, true, "More still folds the rest");
  assert.deepEqual(names(sectionShown(list, { busy: new Set(["c7"]) }).shown), ["c8", "c7", "c6", "c5", "c4"], "one among the first five adds none");
});

test("a chat whose own turn is over while a thread it asked is still out waits on that agent: its row says so, and it is never folded (Codex on #977)", () => {
  const list = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => chat(`c${n}`, n * 10));
  const reader = { fragment: "reader--a1b2" };
  // c1, folded and not open: its agent's turn ended (nothing doing there), its thread to Reader not answered
  const statuses = new Map(list.map((f) => [f.name, rowStatus([], f.name === "c1" ? reader : null)]));
  assert.deepEqual(statuses.get("c1"), { waits: reader, seen: [] });
  const busy = new Set([...statuses].filter(([, st]) => st).map(([name]) => name));
  assert.deepEqual(names(sectionShown(list, { busy }).shown), ["c8", "c7", "c6", "c5", "c4", "c1"]);
  // its own agent doing something there says that first, whatever it waits on
  const working = { chat: "c1", doing: "working", at: 1, until: 2 };
  assert.deepEqual(rowStatus([working], reader), { doing: working, seen: [working] });
  assert.equal(rowStatus([], null), null);
});

test("five or fewer: all of them, and no More", () => {
  const five = [1, 2, 3, 4, 5].map((n) => chat(`c${n}`, n));
  assert.deepEqual(sectionShown(five), { shown: [...five].reverse(), more: false });
  assert.deepEqual(sectionShown([]), { shown: [], more: false });
});
