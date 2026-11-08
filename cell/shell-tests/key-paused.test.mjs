// cell/shell/computer.js's paused shared key's notice (FIN-448):
// `keyPausedSaid`, its words, and `createExpiry`, which draws an idle page
// again as a pause ends, driven by a fake clock.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createExpiry, keyPausedSaid } from "../shell/computer.js";

const paused = (o) => ({ kind: "key_paused", provider: "elevenlabs", spentMicros: 5_030_000, capMicros: 5_000_000, resumesAt: 60_000, ...o });

test("a pause: the key, and what it spent this hour; none past its end", () => {
  assert.deepEqual(keyPausedSaid(paused(), 0), { title: "ElevenLabs calls paused: $5.03 this hour" });
  assert.deepEqual(keyPausedSaid(paused({ spentMicros: 20_000_000 }), 59_999), { title: "ElevenLabs calls paused: $20 this hour" });
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
  const draw = () => {
    shown = notices.map((n) => (n.kind === "key_paused" ? keyPausedSaid(n, c.now()) : n)).filter(Boolean);
    expiry(notices, c.now(), draw);
  };
  draw();
  assert.equal(shown.length, 3);
  assert.deepEqual(c.pending(), [60_000], "one redraw, at the earliest end");
  c.tick(60_000);
  assert.deepEqual(shown.map((n) => n.title ?? n.kind), ["ElevenLabs calls paused: $5.03 this hour", "update"]);
  assert.deepEqual(c.pending(), [90_000], "then the next end");
  c.tick(30_000);
  assert.deepEqual(shown.map((n) => n.title ?? n.kind), ["update"], "both gone");
  assert.deepEqual(c.pending(), [], "nothing left to wait for");
});
