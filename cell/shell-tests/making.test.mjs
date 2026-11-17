// cell/shell/making.js: the chat a New chat is making, one state and one
// `step`. The table walks every phase with every event: each pair it takes
// lands where the table says, and every other pair throws (the shell's
// mistake, never silently nothing). Valid: started, answered, listed,
// left, renamed. Invalid: an answer twice, a start over one in flight.
// Replayed: a second leave, a list read without its chat. Restarted: a
// reload is none, the list the truth.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { step, PHASES, EVENTS, MakingError, owns, holdsView, shownChat, readChat, ownRow, drawName } from "../shell/making.js";

const NAME = "reader-chat--bbbb";
const before = { page: null, current: "talk--aaaa" };
// the New chat that started it (asks.js `hold("start")`'s token), and another
const BY = Object.freeze({ kind: "start", key: "" });
const OTHER = Object.freeze({ kind: "start", key: "" });
const started = { type: "start", by: BY, agent: "npub1reader", name: NAME, title: "New chat", before };
const at = {
  none: null,
  shown: step(null, started).next,
  away: step(step(null, started).next, { type: "left" }).next,
  made: step(step(null, started).next, { type: "answered", by: BY, outcome: "made", listed: false }).next,
};
// each event, with each payload it comes with
const events = {
  start: [started],
  answered: [
    { type: "answered", by: BY, outcome: "made", listed: true },
    { type: "answered", by: BY, outcome: "made", listed: false },
    { type: "answered", by: BY, outcome: "refused" },
  ],
  listed: [
    { type: "listed", has: (name) => name === NAME },
    { type: "listed", has: () => false },
  ],
  left: [{ type: "left" }],
  renamed: [{ type: "renamed", by: BY, name: "reader-chat--dddd" }],
  reload: [{ type: "reload" }],
};
const phase = (m) => m?.phase ?? "none";
const key = (e) => [e.type, e.outcome, e.listed, e.type === "listed" ? e.has(NAME) : undefined].filter((x) => x !== undefined).join(" ");
// phase → event → [next phase, then]; a pair not here throws
const TABLE = {
  none: { start: ["shown", "stay"], "listed true": ["none", "stay"], "listed false": ["none", "stay"], left: ["none", "stay"], reload: ["none", "stay"] },
  shown: {
    "answered made true": ["none", "open"],
    "answered made false": ["made", "wait"],
    "answered refused": ["none", "restore"],
    "listed true": ["shown", "stay"],
    "listed false": ["shown", "stay"],
    left: ["away", "stay"],
    renamed: ["shown", "stay"],
    reload: ["none", "stay"],
  },
  away: {
    "answered made true": ["none", "stay"],
    "answered made false": ["none", "stay"],
    "answered refused": ["none", "stay"],
    "listed true": ["away", "stay"],
    "listed false": ["away", "stay"],
    left: ["away", "stay"],
    renamed: ["away", "stay"],
    reload: ["none", "stay"],
  },
  made: { "listed true": ["none", "open"], "listed false": ["made", "stay"], left: ["none", "stay"], reload: ["none", "stay"] },
};

test("every phase with every event: each pair lands where the table says, and every other pair throws", () => {
  assert.deepEqual(PHASES, Object.keys(at));
  assert.deepEqual(EVENTS.toSorted(), Object.keys(events).toSorted());
  let walked = 0;
  for (const p of PHASES) {
    for (const e of Object.values(events).flat()) {
      const want = TABLE[p][key(e)];
      walked++;
      if (!want) {
        assert.throws(() => step(at[p], e), MakingError, `${key(e)} while ${p}`);
        continue;
      }
      const { next, then } = step(at[p], e);
      assert.deepEqual([phase(next), then], want, `${key(e)} while ${p}`);
    }
  }
  assert.equal(walked, PHASES.length * Object.values(events).flat().length);
});

test("started, it is shown: its row is the active one, and no chat's messages are on screen", () => {
  const m = at.shown;
  assert.equal(holdsView(m), true);
  assert.equal(shownChat(m, "talk--aaaa", null), NAME);
  assert.equal(shownChat(m, "talk--aaaa", "Settings"), NAME);
  assert.equal(readChat(m, "talk--aaaa", null), null);
  assert.deepEqual(m.before, before);
});

test("away, the view given since is the one on screen, read and active", () => {
  const m = at.away;
  assert.equal(holdsView(m), false);
  assert.equal(shownChat(m, "other--cccc", null), "other--cccc");
  assert.equal(readChat(m, "other--cccc", null), "other--cccc");
  assert.equal(shownChat(m, "other--cccc", "Settings"), null);
  assert.equal(readChat(m, "other--cccc", "Settings"), null);
});

test("made, waiting for its list: its room keeps the view and reads nothing; the read that has it opens it", () => {
  const m = at.made;
  assert.equal(holdsView(m), true);
  assert.equal(shownChat(m, "talk--aaaa", null), NAME);
  assert.equal(readChat(m, "talk--aaaa", null), null);
  // replayed: a read without it, again and again, changes nothing
  let again = m;
  for (let i = 0; i < 3; i++) again = step(again, { type: "listed", has: () => false }).next;
  assert.deepEqual(again, m);
  assert.deepEqual(step(again, { type: "listed", has: (name) => name === NAME }), { next: null, then: "open" });
});

test("renamed (its name taken), it keeps its phase and its view before, under its new name", () => {
  for (const p of ["shown", "away"]) {
    const { next } = step(at[p], { type: "renamed", by: BY, name: "reader-chat--dddd" });
    assert.equal(next.phase, p);
    assert.equal(next.name, "reader-chat--dddd");
    assert.deepEqual(next.before, before);
  }
});

test("each New chat owns its own: another's answer or rename never reaches it, and a start names its owner", () => {
  for (const p of ["shown", "away"]) {
    assert.equal(owns(at[p], BY), true);
    assert.equal(owns(at[p], OTHER), false, "a token alike is not the same token");
    for (const e of [
      { type: "answered", by: OTHER, outcome: "made", listed: true },
      { type: "answered", by: OTHER, outcome: "refused" },
      { type: "answered", outcome: "made", listed: true },
      { type: "renamed", by: OTHER, name: "reader-chat--eeee" },
    ]) assert.throws(() => step(at[p], e), MakingError, `${e.type} from another New chat while ${p}`);
  }
  // an earlier one waiting for its list takes no answer at all, its own or
  // another's: a later New chat ends it by giving the column a view (`left`)
  assert.throws(() => step(at.made, { type: "answered", by: OTHER, outcome: "made", listed: true }), MakingError);
  assert.deepEqual(step(at.made, { type: "left" }), { next: null, then: "stay" });
  assert.equal(owns(null, BY), false);
  assert.equal(owns(at.shown, null), false);
  assert.throws(() => step(null, { ...started, by: null }), MakingError, "a start names its owner");
});

test("its own row is drawn until the list has its name; another device's chat is not it", () => {
  assert.equal(ownRow(at.shown, () => false), true);
  assert.equal(ownRow(at.shown, (name) => name === NAME), false);
  assert.equal(ownRow(at.shown, (name) => name === "reader-chat--zzzz"), true);
  assert.equal(ownRow(null, () => false), false);
  assert.equal(readChat(null, "talk--aaaa", null), "talk--aaaa");
  assert.equal(shownChat(null, "talk--aaaa", "Apps"), null);
});

test("a drawn name is its label, two dashes, and four of the platform's suffix letters", () => {
  for (let i = 0; i < 50; i++) assert.match(drawName("reader-chat"), /^reader-chat--[a-km-np-z2-9]{4}$/);
});
