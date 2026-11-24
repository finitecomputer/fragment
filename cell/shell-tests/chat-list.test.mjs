// cell/shell/chat-list.js's sections (Paul, 2026-10-31): a section's
// order, its pinned chats first by title, holding their places as
// messages come (FIN-722), then the rest, the newest first; and what it
// shows, its first five, every pinned one, the open one wherever it is,
// each whose row says what its agent is doing (never folded under More),
// and all of them unfolded.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { pinnedAgentsFirst, rowStatus, sectionOrder, sectionShown } from "../shell/chat-list.js";

const chat = (name, previewAt, pinned = false, title = name) => ({ name, previewAt, pinned, title });
const names = (list) => list.map((f) => f.name);
const byTitle = sectionOrder((f) => f.title);

test("pinned chats come first, by title, then the rest, the newest first", () => {
  const list = [chat("old", 1), chat("pinned-b", 2, true, "Board"), chat("new", 9), chat("pinned-a", 5, true, "agenda"), chat("made-now", undefined)];
  assert.deepEqual(names([...list].sort(byTitle)), ["pinned-a", "pinned-b", "made-now", "new", "old"]);
});

test("pinned chats hold their places as messages come; two of one title go by name; numbers in order", () => {
  const before = [chat("x", 1, true, "Email blasts"), chat("y", 2, true, "Board"), chat("z", 3, true, "Newsletter")];
  const after = before.map((f) => (f.name === "z" ? { ...f, previewAt: 99 } : f.name === "x" ? { ...f, previewAt: 50 } : f));
  assert.deepEqual(names([...before].sort(byTitle)), ["y", "x", "z"]);
  assert.deepEqual(names([...after].sort(byTitle)), ["y", "x", "z"], "a new message moves none");
  assert.deepEqual(names([chat("b", 2, true, "Notes"), chat("a", 1, true, "Notes")].sort(byTitle)), ["a", "b"]);
  assert.deepEqual(names([chat("ten", 1, true, "10 Plans"), chat("two", 1, true, "2 Plans")].sort(byTitle)), ["two", "ten"]);
});

test("a section shows its first five, pinned ones counted among them, and More past them", () => {
  const list = [1, 2, 3, 4, 5, 6, 7].map((n) => chat(`c${n}`, n * 10)).concat([chat("p1", 1, true), chat("p2", 2, true)]);
  const { shown, more } = sectionShown(list);
  assert.deepEqual(names(shown), ["p1", "p2", "c7", "c6", "c5"]);
  assert.equal(more, true);
});

test("every pinned chat shows, however many, never folded under More (FIN-722)", () => {
  const pinned = [1, 2, 3, 4, 5, 6, 7].map((n) => chat(`p${n}`, n, true));
  const list = pinned.concat([chat("c1", 100)]);
  assert.deepEqual(sectionShown(list), { shown: [...pinned], more: true }, "the unpinned one is folded");
  assert.deepEqual(sectionShown(pinned), { shown: [...pinned], more: false }, "pinned alone: nothing to fold, no More");
  assert.deepEqual(names(sectionShown(list, { title: (f) => (f.name === "p7" ? "A first" : f.title) }).shown).slice(0, 2), ["p7", "p1"], "by the titles the rows show");
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

test("the sidebar's agents: the pinned ones first, then the rest, each in their computer's order; none left out (FIN-404)", () => {
  const agent = (fragment) => ({ fragment, identity: `id-${fragment}` });
  const mine = ["main", "b", "c", "d"].map(agent);
  const order = (pinned) => pinnedAgentsFirst(mine, (name) => pinned.includes(name)).map((a) => a.fragment);
  assert.deepEqual(order([]), ["main", "b", "c", "d"], "nothing pinned: their computer's order");
  assert.deepEqual(order(["c"]), ["c", "main", "b", "d"]);
  assert.deepEqual(order(["d", "b"]), ["b", "d", "main", "c"], "pinned ones keep their computer's order among them");
  assert.deepEqual(order(["main", "b", "c", "d"]), ["main", "b", "c", "d"]);
  assert.deepEqual(order(["gone"]), ["main", "b", "c", "d"], "a pin of an agent no longer theirs moves none");
  assert.deepEqual(mine.map((a) => a.fragment), ["main", "b", "c", "d"], "their computer's order itself is untouched");
});
