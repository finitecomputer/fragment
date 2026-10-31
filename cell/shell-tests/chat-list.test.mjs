// cell/shell/chat-list.js's sections (Paul, 2026-10-31): a section's
// order, its pinned chats first, then the rest, each the newest first; and
// what it shows, its first five (pinned ones among them), the open one
// wherever it is, and all of them unfolded.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { pinnedFirst, sectionShown } from "../shell/chat-list.js";

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

test("five or fewer: all of them, and no More", () => {
  const five = [1, 2, 3, 4, 5].map((n) => chat(`c${n}`, n));
  assert.deepEqual(sectionShown(five), { shown: [...five].reverse(), more: false });
  assert.deepEqual(sectionShown([]), { shown: [], more: false });
});
