// cell/shell/restart-follow.js: a restart the person asked for, followed
// until the computer says it is done, as one explicit state (FIN-395),
// driven with a fake clock and reads and asks held until each test
// answers them (Codex on #1053: it read on hidden and after disposal, said
// "Starting…" over failed reads, and took a failed ask for done).
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createRestartFollow, FOLLOW_MS, FOLLOWING, PAUSED, READ_FAILED, FAILED } from "../shell/restart-follow.js";

const ID = "computer:aa";
const at = (generation, phase) => ({ computer: ID, generation, phase });
// every promise callback queued so far, run
const settle = () => new Promise((r) => setImmediate(r));

// The page: its computer's view, its timers (a fake clock), and each read
// and ask held until answered.
// `drawChecks`: drawing a view taken asks whether it is done, as
// computer.js's does (`computerIs`, then `renderComputer`'s `check`).
function page({ generation = 4, phase = "awake", hidden = false, drawChecks = false } = {}) {
  let clock = 0;
  let next = 1;
  const due = new Map();
  const timers = {
    setTimeout(fn, ms) {
      due.set(next, { at: clock + ms, fn });
      return next++;
    },
    clearTimeout(id) {
      due.delete(id);
    },
  };
  const held = (list, args) => new Promise((resolve, reject) => list.push({ resolve, reject, args }));
  const p = {
    view: at(generation, phase),
    hidden,
    reads: [],
    asks: [],
    changes: 0,
    timers: () => due.size,
    // the clock moved on `ms`, each timer due by then fired
    async tick(ms = FOLLOW_MS) {
      clock += ms;
      for (const [id, t] of [...due]) {
        if (t.at <= clock) {
          due.delete(id);
          t.fn();
        }
      }
      await settle();
    },
    // the oldest read still held, answered with `v` (or failed with `e`)
    async answerRead(v, e) {
      const r = p.reads.shift();
      assert.ok(r, "a read was asked");
      e ? r.reject(e) : r.resolve(v);
      await settle();
    },
    async answerAsk(v, e) {
      const a = p.asks.shift();
      assert.ok(a, "the restart was asked");
      e ? a.reject(e) : a.resolve(v);
      await settle();
    },
  };
  p.follow = createRestartFollow({
    read: () => held(p.reads),
    ask: (asked) => held(p.asks, asked),
    view: () => p.view,
    take: (v) => {
      p.view = v;
      if (drawChecks) p.follow.check();
    },
    changed: () => p.changes++,
    hidden: () => p.hidden,
    timers,
    now: () => clock,
  });
  return p;
}

test("following: its view read every two seconds, the restart's answer starting, then awake on the newer start: done, no timer left", async () => {
  const p = page();
  assert.equal(p.follow.press(), true);
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: FOLLOWING, error: null });
  assert.equal(p.asks[0].args, 4, "it names the start it restarts");
  assert.equal(p.follow.press(), false, "pressed again while it runs: nothing more");
  await p.tick();
  await p.answerRead(at(4, "sleeping"));
  assert.equal(p.view.phase, "sleeping");
  await p.answerAsk(at(4, "starting"));
  await p.tick();
  await p.answerRead(at(5, "starting"));
  assert.equal(p.follow.op().state, FOLLOWING, "starting is no end");
  await p.tick();
  await p.answerRead(at(5, "awake"));
  assert.equal(p.follow.op(), null, "awake on the newer start: done");
  assert.equal(p.timers(), 0);
  await p.tick();
  assert.equal(p.reads.length, 0, "nothing read after");
});

test("hidden: paused, nothing read and no timer, a read under way dropped; shown again: read at once, then every two seconds", async () => {
  const p = page();
  p.follow.press();
  await p.tick();
  assert.equal(p.reads.length, 1);
  p.hidden = true;
  p.follow.visibility();
  assert.equal(p.follow.op().state, PAUSED);
  assert.equal(p.timers(), 0);
  await p.answerRead(at(4, "sleeping"));
  assert.equal(p.view.phase, "awake", "the read under way as it hid is dropped");
  await p.tick(60_000);
  assert.equal(p.reads.length, 0, "nothing read while hidden");
  p.hidden = false;
  p.follow.visibility();
  assert.equal(p.follow.op().state, FOLLOWING);
  assert.equal(p.reads.length, 1, "read at once");
  await p.answerRead(at(5, "starting"));
  assert.equal(p.timers(), 1, "and again in two seconds");
});

test("pressed while hidden: paused from the first, its ask still sent", async () => {
  const p = page({ hidden: true });
  p.follow.press();
  assert.equal(p.follow.op().state, PAUSED);
  assert.equal(p.asks.length, 1);
  assert.equal(p.timers(), 0);
  await p.answerAsk(at(4, "starting"));
  assert.equal(p.timers(), 0, "its answer starts no reads while hidden");
});

test("disposed: no timer, a read under way dropped, nothing read after, its ask's answer ignored", async () => {
  const p = page();
  p.follow.press();
  await p.tick();
  p.follow.dispose();
  assert.equal(p.follow.op(), null);
  assert.equal(p.timers(), 0);
  const changes = p.changes;
  await p.answerRead(at(5, "starting"));
  await p.answerAsk(at(4, "starting"));
  assert.equal(p.view.phase, "awake", "nothing taken");
  assert.equal(p.changes, changes, "nothing drawn");
  await p.tick(60_000);
  assert.equal(p.reads.length, 0);
});

test("a read that fails: said as the read's failure (no news of the restart), nothing read on; Check again reads at once and goes on", async () => {
  const p = page();
  p.follow.press();
  await p.answerAsk(at(4, "starting"));
  await p.tick();
  await p.answerRead(null, new Error("Failed to fetch"));
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: READ_FAILED, error: "Failed to fetch" });
  assert.equal(p.timers(), 0);
  await p.tick(60_000);
  assert.equal(p.reads.length, 0, "nothing read while it says it can't");
  p.follow.readAgain();
  assert.equal(p.follow.op().state, FOLLOWING);
  assert.equal(p.reads.length, 1, "read at once");
  await p.answerRead(at(5, "awake"));
  assert.equal(p.follow.op(), null, "done");
});

test("its ask answering after a read failed: following again (an answer is a read that worked)", async () => {
  const p = page();
  p.follow.press();
  await p.tick();
  await p.answerRead(null, new Error("Failed to fetch"));
  assert.equal(p.follow.op().state, READ_FAILED);
  await p.answerAsk(at(4, "starting"));
  assert.equal(p.follow.op().state, FOLLOWING);
  assert.equal(p.timers(), 1);
});

test("its ask failing once a read saw the newer start: no end, and no failure; it goes on until awake", async () => {
  const p = page();
  p.follow.press();
  await p.tick();
  await p.answerRead(at(5, "starting"));
  await p.answerAsk(null, new Error("the connection was lost"));
  assert.equal(p.reads.length, 1, "one read says whether it is under way");
  await p.answerRead(at(5, "starting"));
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: FOLLOWING, error: null }, "under way: still followed");
  await p.tick();
  await p.answerRead(at(5, "awake"));
  assert.equal(p.follow.op(), null);
});

test("its ask refused and the computer showing no restart: failed, its words said, nothing read on; Restart again begins another", async () => {
  const p = page();
  p.follow.press();
  await p.answerAsk(null, new Error("agents are stopped: the credit is used up"));
  await p.answerRead(at(4, "awake"));
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: FAILED, error: "agents are stopped: the credit is used up" });
  assert.equal(p.timers(), 0);
  assert.equal(p.follow.press(), true, "Try again");
  assert.equal(p.follow.op().state, FOLLOWING);
  assert.equal(p.asks.length, 1);
});

test("its ask failing and the read after it failing too: the read's failure said, never that it did not restart", async () => {
  const p = page();
  p.follow.press();
  await p.answerAsk(null, new Error("Failed to fetch"));
  await p.answerRead(null, new Error("offline"));
  assert.equal(p.follow.op().state, READ_FAILED);
  assert.equal(p.follow.op().error, "offline");
});

test("the restart's answer is newer than a read asked before it: that read is dropped, never back to saving", async () => {
  const p = page();
  p.follow.press();
  await p.tick();
  await p.answerAsk(at(4, "starting"));
  await p.answerRead(at(4, "sleeping"));
  assert.equal(p.view.phase, "starting");
});

test("done by a view the page took elsewhere (its list's read), awake on a newer start or a newer start that won't start; not by one still starting", async () => {
  for (const phase of ["awake", "wont_wake"]) {
    const p = page();
    p.follow.press();
    p.view = at(5, "starting");
    assert.equal(p.follow.check(), false);
    p.view = at(5, phase);
    assert.equal(p.follow.check(), true);
    assert.equal(p.follow.op(), null);
    assert.equal(p.timers(), 0);
  }
  const p = page({ generation: 4, phase: "wont_wake" });
  p.follow.press();
  assert.equal(p.follow.check(), false, "won't start on the start it restarts: not done");
});

test("found done by the page's own drawing of the view it takes (a read's, or the answer's): no timer left behind", async () => {
  const p = page({ drawChecks: true });
  p.follow.press();
  await p.tick();
  await p.answerRead(at(5, "awake"));
  assert.equal(p.follow.op(), null);
  assert.equal(p.timers(), 0);
  const q = page({ drawChecks: true, phase: "wont_wake" });
  q.follow.press();
  await q.answerAsk(at(5, "wont_wake"));
  assert.equal(q.follow.op(), null, "a newer start that won't start: done, its notice says so");
  assert.equal(q.timers(), 0);
});
