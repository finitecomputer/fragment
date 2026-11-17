// cell/shell/asks.js: what a late answer may still do. Each kind has one
// latest ask, and an answer counts only while its ask is that one. Valid:
// answers in order. Invalid: an answer out of order, or after a leave
// (another ask of its kind), counts for nothing; an unknown kind throws.
// Replayed: an answer that came twice counts as often as its ask is
// current, and changes nothing else. Retried: a failure followed by its
// retry, the retry's answer alone counts. Held: one at a time by key.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createAsks, KINDS, HOLDS } from "../shell/asks.js";

// what an answer does: kept only while its ask is current
function answering(asks) {
  const shown = [];
  return { shown, answer: (token, value) => (asks.current(token) ? shown.push(value) : null) };
}

test("answers in order: each counts while its ask is the latest", () => {
  for (const kind of KINDS) {
    const asks = createAsks();
    const { shown, answer } = answering(asks);
    const a = asks.ask(kind);
    answer(a, "a");
    const b = asks.ask(kind);
    answer(b, "b");
    assert.deepEqual(shown, ["a", "b"], kind);
  }
});

test("out of order: an older ask's answer, after a newer was asked, counts for nothing", () => {
  const asks = createAsks();
  const { shown, answer } = answering(asks);
  const a = asks.ask("settings-load");
  const b = asks.ask("settings-load");
  answer(b, "b");
  answer(a, "a (late)");
  assert.deepEqual(shown, ["b"]);
});

test("after a leave: another ask of its kind (a view given, a visit ended) and an answer begun before counts no more", () => {
  const asks = createAsks();
  const { shown, answer } = answering(asks);
  const launch = asks.now("view");
  asks.ask("view"); // the person opened another chat
  answer(launch, "the launch's chat");
  assert.deepEqual(shown, []);
  // a visit's close is an ask too: the open's late reads are dropped
  const visit = asks.ask("apps");
  asks.ask("apps");
  assert.equal(asks.current(visit), false);
});

test("kinds are apart: an ask of one never makes another's answer late", () => {
  const asks = createAsks();
  const tokens = KINDS.map((kind) => asks.ask(kind));
  for (const kind of KINDS) asks.ask(kind === "notice" ? "view" : "notice");
  assert.equal(asks.current(tokens[KINDS.indexOf("computer")]), true);
  assert.equal(asks.current(tokens[KINDS.indexOf("apps")]), true);
});

test("now asks nothing: an answer begun under it counts until the next ask, as a read the action's own answer outdates", () => {
  const asks = createAsks();
  const read = asks.now("computer");
  assert.equal(asks.current(read), true);
  assert.equal(asks.now("computer"), read, "the same token, so a token kept elsewhere compares as itself");
  asks.ask("computer"); // an action's answer took a view of the computer
  assert.equal(asks.current(read), false);
  assert.equal(asks.now("view").n, 0, "numbered from none (the launcher's `views()`)");
});

test("replayed: an answer that comes twice counts as its ask does, and changes nothing else", () => {
  const asks = createAsks();
  const { shown, answer } = answering(asks);
  const a = asks.ask("notice");
  answer(a, "said");
  answer(a, "said");
  assert.deepEqual(shown, ["said", "said"]);
  assert.equal(asks.current(a), true, "answering asks nothing");
});

test("a failure, then its retry: the failed ask's late answer counts for nothing, the retry's does", () => {
  const asks = createAsks();
  const { shown, answer } = answering(asks);
  const first = asks.ask("settings-load");
  const retry = asks.ask("settings-load"); // its failure said, asked again
  answer(first, "the failed read, late");
  answer(retry, "the retry");
  assert.deepEqual(shown, ["the retry"]);
});

test("held one at a time by key: a second hold while one is in flight gets nothing; let go, it may be held again", () => {
  const asks = createAsks();
  const start = asks.hold("start");
  assert.ok(start);
  assert.equal(asks.hold("start"), null);
  assert.equal(asks.held("start"), true);
  asks.letGo(start);
  assert.equal(asks.held("start"), false);
  assert.ok(asks.hold("start"));
  // by key: two chats titled at once, each once
  const a = asks.hold("title", "a--aaaa");
  assert.ok(asks.hold("title", "b--bbbb"));
  assert.equal(asks.hold("title", "a--aaaa"), null);
  asks.letGo(a);
  asks.letGo(a); // replayed: nothing more
  asks.letGo(null);
  assert.equal(asks.held("title", "a--aaaa"), false);
  assert.equal(asks.held("title", "b--bbbb"), true);
});

test("an unknown kind is the shell's mistake: it throws", () => {
  const asks = createAsks();
  assert.throws(() => asks.ask("views"), RangeError);
  assert.throws(() => asks.now("list"), RangeError);
  assert.throws(() => asks.hold("view"), RangeError);
  assert.equal(asks.current(null), false);
  assert.ok(HOLDS.every((kind) => asks.hold(kind)));
});
