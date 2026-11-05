// cell/shell/making.js: the chat a New chat is making, its one state and
// its moves. Valid: started, it is the chat on screen; another view given,
// it is away and its row stays; settled, it opens or restores only while
// shown. Invalid: a settle or a leave with none is nothing. Replayed: a
// second leave changes nothing; its row goes once the list has its name.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { start, renamed, left, settled, shownChat, ownRow, drawName } from "../shell/making.js";

const before = { page: null, current: "talk--aaaa" };
const made = () => start({ agent: "npub1reader", name: "reader-chat--bbbb", title: "New chat", before });

test("started, it is shown: the chat on screen is it, not the chat open before, nor a page", () => {
  const m = made();
  assert.equal(m.phase, "shown");
  assert.equal(shownChat(m, "talk--aaaa", null), "reader-chat--bbbb");
  assert.equal(shownChat(m, "talk--aaaa", "Settings"), "reader-chat--bbbb");
  assert.deepEqual(m.before, before);
});

test("another view given, it is away: the chat on screen is that view's, and a second leave changes nothing", () => {
  const away = left(made());
  assert.equal(away.phase, "away");
  assert.equal(shownChat(away, "other--cccc", null), "other--cccc");
  assert.equal(shownChat(away, "other--cccc", "Settings"), null);
  assert.deepEqual(left(away), away);
});

test("settled while shown: made opens its chat, refused brings the view before back", () => {
  assert.equal(settled(made(), "made"), "open");
  assert.equal(settled(made(), "refused"), "restore");
});

test("settled while away: nothing moves, made or refused", () => {
  assert.equal(settled(left(made()), "made"), "stay");
  assert.equal(settled(left(made()), "refused"), "stay");
});

test("none: no chat being made shows, leaves, settles or draws a row", () => {
  assert.equal(left(null), null);
  assert.equal(renamed(null, "x--aaaa"), null);
  assert.equal(settled(null, "made"), "stay");
  assert.equal(shownChat(null, "talk--aaaa", null), "talk--aaaa");
  assert.equal(shownChat(null, "talk--aaaa", "Apps"), null);
  assert.equal(ownRow(null, () => false), false);
});

test("its own row is drawn until the list has its name, whose row then stands in its place", () => {
  const m = made();
  assert.equal(ownRow(m, () => false), true);
  assert.equal(ownRow(m, (name) => name === "reader-chat--bbbb"), false);
  // another device's chat, made at the same time, is not it
  assert.equal(ownRow(m, (name) => name === "reader-chat--zzzz"), true);
});

test("renamed (its name taken), it keeps its phase and its view before, under its new name", () => {
  const r = renamed(left(made()), "reader-chat--dddd");
  assert.equal(r.phase, "away");
  assert.equal(r.name, "reader-chat--dddd");
  assert.deepEqual(r.before, before);
});

test("a drawn name is its label, two dashes, and four of the platform's suffix letters", () => {
  for (let i = 0; i < 50; i++) assert.match(drawName("reader-chat"), /^reader-chat--[a-km-np-z2-9]{4}$/);
});
