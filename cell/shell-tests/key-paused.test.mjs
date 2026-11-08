// cell/shell/computer.js's paused key's notice (FIN-448; Alex on #1117):
// `keyPausedSaid`, its words and its one tap, which offers only what the
// platform says (`allowMicros`, never more than a person may allow) and,
// at the most, says so with no button; and `createExpiry`, which draws an
// idle page again as a pause ends, driven by a fake clock.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createExpiry, keyPausedSaid } from "../shell/computer.js";

const USD = 1_000_000;
const paused = (o) => ({ kind: "key_paused", provider: "elevenlabs", spentMicros: 5_030_000, capMicros: 5 * USD, allowMicros: 20 * USD, resumesAt: 60_000, ...o });

test("a pause: what it spent this hour, and the tap the platform offers", () => {
  assert.deepEqual(keyPausedSaid(paused(), 0), { title: "ElevenLabs calls paused: $5.03 this hour", allow: "Allow up to $20 an hour", most: null });
});

test("caps above $250: the offer is the platform's, the most a person may allow, never four times", () => {
  for (const cap of [251, 320, 999]) {
    const said = keyPausedSaid(paused({ spentMicros: cap * USD, capMicros: cap * USD, allowMicros: 1_000 * USD }), 0);
    assert.equal(said.allow, "Allow up to $1,000 an hour", `${cap}`);
    assert.equal(said.most, null);
  }
});

test("at the most: said plainly, and no button", () => {
  const said = keyPausedSaid(paused({ spentMicros: 1_000_400_000, capMicros: 1_000 * USD, allowMicros: undefined }), 0);
  assert.deepEqual(said, { title: "ElevenLabs calls paused: $1,000.40 this hour", allow: null, most: "$1,000 an hour is the most allowed." });
});

test("past its end, no pause is said", () => {
  assert.equal(keyPausedSaid(paused(), 59_999) === null, false);
  assert.equal(keyPausedSaid(paused(), 60_000), null);
});

// The page's clock and timers, a test's.
function clock() {
  let now = 0;
  let next = 1;
  const due = new Map();
  return {
    now: () => now,
    pending: () => [...due.values()].map((t) => t.at),
    setTimeout(fn, ms) {
      due.set(next, { at: now + ms, fn });
      return next++;
    },
    clearTimeout(id) {
      due.delete(id);
    },
    tick(ms) {
      now += ms;
      for (const [id, t] of [...due]) {
        if (t.at <= now) {
          due.delete(id);
          t.fn();
        }
      }
    },
  };
}

test("an idle page drops a pause as it ends: one redraw at the earliest end, no call or event", () => {
  const c = clock();
  const expiry = createExpiry(c);
  const notices = [paused({ resumesAt: 90_000 }), paused({ provider: "xai", resumesAt: 60_000 }), { kind: "update", version: "abc" }];
  let shown = [];
  let draws = 0;
  const draw = () => {
    draws += 1;
    shown = notices.map((n) => (n.kind === "key_paused" ? keyPausedSaid(n, c.now()) : n)).filter(Boolean);
    expiry(notices, c.now(), draw);
  };
  draw();
  assert.equal(shown.length, 3);
  assert.deepEqual(c.pending(), [60_000], "one redraw, at the earliest end");
  c.tick(59_999);
  assert.equal(draws, 1, "nothing before it");
  c.tick(1);
  assert.equal(draws, 2, "drawn again as xai's ends");
  assert.deepEqual(shown.map((n) => n.title ?? n.kind), ["ElevenLabs calls paused: $5.03 this hour", "update"]);
  assert.deepEqual(c.pending(), [90_000], "then the next end");
  c.tick(30_000);
  assert.deepEqual(shown.map((n) => n.title ?? n.kind), ["update"], "both gone, with no call or event");
  assert.deepEqual(c.pending(), [], "and nothing left to wait for");
});

test("each drawing replaces the redraw before it, never adding one", () => {
  const c = clock();
  const expiry = createExpiry(c);
  expiry([paused({ resumesAt: 60_000 })], c.now(), () => {});
  c.tick(10_000);
  expiry([paused({ resumesAt: 40_000 })], c.now(), () => {});
  assert.deepEqual(c.pending(), [40_000]);
  c.tick(10_000);
  expiry([], c.now(), () => {});
  assert.deepEqual(c.pending(), [], "no pause: no redraw");
});
