// cell/shell/making.js: the chat a New chat is making, its one state and
// its moves. Valid: started, it is the chat on screen; another view given,
// it is away and its row stays; settled, it opens or restores only while
// shown, or waits in its room for the list; no chat's messages are on
// screen while its room is. Invalid: a settle or a leave with none is
// nothing. Replayed: a second leave changes nothing; its row goes once the
// list has its name.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { start, renamed, left, settled, listed, holdsView, shownChat, readChat, ownRow, drawName } from "../shell/making.js";

const before = { page: null, current: "talk--aaaa" };
const made = () => start({ agent: "npub1reader", name: "reader-chat--bbbb", title: "New chat", before });

test("started, it is shown: its row is the active one, and no chat's messages are on screen", () => {
  const m = made();
  assert.equal(m.phase, "shown");
  assert.equal(holdsView(m), true);
  assert.equal(shownChat(m, "talk--aaaa", null), "reader-chat--bbbb");
  assert.equal(shownChat(m, "talk--aaaa", "Settings"), "reader-chat--bbbb");
  // its room shows no messages: neither the chat it covers nor its own
  assert.equal(readChat(m, "talk--aaaa", null), null);
  assert.deepEqual(m.before, before);
});

test("another view given, it is away: the chat on screen is that view's, and a second leave changes nothing", () => {
  const away = left(made());
  assert.equal(away.phase, "away");
  assert.equal(holdsView(away), false);
  assert.equal(shownChat(away, "other--cccc", null), "other--cccc");
  assert.equal(readChat(away, "other--cccc", null), "other--cccc");
  assert.equal(shownChat(away, "other--cccc", "Settings"), null);
  assert.equal(readChat(away, "other--cccc", "Settings"), null);
  assert.deepEqual(left(away), away);
});

test("settled while shown: made and listed opens its chat, refused brings the view before back", () => {
  assert.deepEqual(settled(made(), "made", true), { next: null, then: "open" });
  assert.deepEqual(settled(made(), "refused", true), { next: null, then: "restore" });
  assert.deepEqual(settled(made(), "refused", false), { next: null, then: "restore" });
});

test("made before the list has its chat: its room waits, then the list's read that has it opens it", () => {
  const { next, then } = settled(made(), "made", false);
  assert.equal(then, "wait");
  assert.equal(next.phase, "made");
  assert.equal(holdsView(next), true);
  assert.equal(shownChat(next, "talk--aaaa", null), "reader-chat--bbbb");
  assert.equal(readChat(next, "talk--aaaa", null), null);
  // a read without it (a deploy going out) changes nothing; one with it opens
  assert.equal(listed(next, () => false), "stay");
  assert.equal(listed(next, (name) => name === "reader-chat--bbbb"), "open");
  // another view given meanwhile: nothing is left to wait for
  assert.equal(left(next), null);
});

test("settled while away: nothing moves, made or refused, listed or not", () => {
  for (const outcome of ["made", "refused"]) for (const isListed of [true, false]) assert.deepEqual(settled(left(made()), outcome, isListed), { next: null, then: "stay" });
  assert.equal(listed(left(made()), () => true), "stay");
});

test("none: no chat being made shows, leaves, settles or draws a row", () => {
  assert.equal(left(null), null);
  assert.equal(renamed(null, "x--aaaa"), null);
  assert.deepEqual(settled(null, "made", true), { next: null, then: "stay" });
  assert.equal(listed(null, () => true), "stay");
  assert.equal(holdsView(null), false);
  assert.equal(shownChat(null, "talk--aaaa", null), "talk--aaaa");
  assert.equal(readChat(null, "talk--aaaa", null), "talk--aaaa");
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
