// cell/shell/computer.js's pre-wake (Paul, 2026-11-18: a computer is
// awake while it is used, then twenty minutes; only real interaction is a
// use): as its person picks a chat or opens the launcher (shell.js calls
// `prewake` for each), the shell races their computer's start with their
// own wake (`POST …/wake`, held the idle wait). Only another such sign
// asks again, one ask a minute at most: nothing the page does on its own
// (a timer, the tab shown again) asks, so a tab left open lets the
// computer sleep. The real `createComputer`, its page faked as far as the
// pre-wake.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createComputer } from "../shell/computer.js";

const COMPUTER = "computer:aa";
const WAKE = `/api/computers/${COMPUTER}/wake`;
const settle = () => new Promise((r) => setTimeout(r, 0));

// The page's clock and timers, a test's: nothing runs unless it ticks.
function clock(start) {
  let now = start;
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

// The shell as far as its computer's pre-wake: its view (`phase`), the first
// run (`setup`), the page shown or hidden, and every call it makes, each
// answered at once with the view (awake, once a wake has asked), or refused
// while `refusing` is set.
function page(t, { phase = "asleep", setup = false, start = 1_700_000_000_000 } = {}) {
  const listeners = [];
  globalThis.document = {
    hidden: false,
    addEventListener: (type, fn) => listeners.push({ type, fn }),
    removeEventListener: (type, fn) => listeners.splice(listeners.findIndex((l) => l.type === type && l.fn === fn) >>> 0, 1),
    getElementById: () => null,
  };
  globalThis.MutationObserver = class {
    observe() {}
    disconnect() {}
  };
  const c = clock(start);
  const shell = { computer: { computer: COMPUTER, phase, generation: 3, notices: [], agents: [] }, setup };
  const calls = [];
  const it = { refusing: null };
  const api = (method, path) => {
    calls.push({ method, path, at: c.now() });
    return it.refusing ? Promise.reject(it.refusing) : Promise.resolve({ ...shell.computer, phase: "awake" });
  };
  const stub = () => ({ append() {}, replaceChildren() {}, dataset: {}, classList: { add() {} } });
  const ui = createComputer({
    api, el: stub, seg: (s) => s, named: (n) => n, section: stub, line: stub, actionLine: stub,
    computer: () => shell.computer, notices: () => [], agents: () => new Map(), computerFailed: () => false, setupPending: () => shell.setup,
    made: () => {}, computerIs: (v) => (shell.computer = v), openScreen: () => {}, markOffer: () => {}, reads: () => ({}), load: async () => {}, openPage: () => {},
    timers: c, now: c.now,
  });
  t.after(() => ui.dispose());
  return Object.assign(it, {
    ui, calls, clock: c, shell,
    // the routes the page posted to its computer, in order
    posted: () => calls.filter((x) => x.method === "POST").map((x) => x.path),
    // the page shown again, or hidden (the browser's `visibilitychange`)
    shown(visible) {
      document.hidden = !visible;
      for (const l of listeners.filter((l) => l.type === "visibilitychange")) l.fn();
    },
  });
}

test("a sign asks the owner's wake", async (t) => {
  const p = page(t);
  p.ui.prewake();
  assert.deepEqual(p.posted(), [WAKE]);
  await settle();
  assert.equal(p.shell.computer.phase, "awake", "its answer is the computer's view, taken");
});

test("a tab left open asks nothing more: no timer of its own renews the pre-wake", (t) => {
  const p = page(t);
  p.ui.prewake();
  assert.deepEqual(p.clock.pending(), [], "it set no timer");
  for (let i = 0; i < 40; i++) p.clock.tick(60_000);
  assert.deepEqual(p.posted(), [WAKE], "forty minutes open, one wake: the computer sleeps twenty minutes after it");
});

test("each sign the person gives asks again, signs within a minute of the last ask one sign, while the computer is awake too", async (t) => {
  const p = page(t);
  p.ui.prewake();
  await settle();
  assert.equal(p.shell.computer.phase, "awake");
  // chats picked in turn: a sign every 20 s for three minutes
  for (let s = 20; s <= 180; s += 20) {
    p.clock.tick(20_000);
    p.ui.prewake();
  }
  const at = p.calls.map((x) => x.at - p.calls[0].at);
  assert.deepEqual(at, [0, 60_000, 120_000, 180_000], "each ask holds it the idle wait, the next sign within it renewing it");
  // and then nothing for as long as the person gives no sign
  p.clock.tick(20 * 60_000);
  assert.equal(p.calls.length, 4);
});

test("the tab shown again is no sign: it asks nothing; and a hidden page asks nothing for a sign either", (t) => {
  const p = page(t);
  p.shown(false);
  p.shown(true);
  assert.deepEqual(p.posted(), [], "back to the tab: the next sign asks, not the tab");
  p.shown(false);
  p.ui.prewake();
  assert.deepEqual(p.posted(), [], "hidden: no one is there");
  p.shown(true);
  p.ui.prewake();
  assert.deepEqual(p.posted(), [WAKE]);
});

test("a computer that won't wake, or one the first run has not set up, is not pre-woken", (t) => {
  const p = page(t, { phase: "wont_wake" });
  p.ui.prewake();
  p.shown(true);
  assert.deepEqual(p.posted(), [], "won't wake: the next start is its owner's to ask for");
  const q = page(t, { setup: true });
  q.ui.prewake();
  assert.deepEqual(q.posted(), [], "the first run starts it once its agent is in its chat");
  q.shell.setup = false;
  q.ui.prewake();
  assert.deepEqual(q.posted(), [WAKE]);
});

test("a wake refused (no credit, a deploy) is quiet, and the next is asked on the next thing the person does", async (t) => {
  const p = page(t);
  p.refusing = Object.assign(new Error("no credit"), { status: 402 });
  p.ui.prewake();
  await settle();
  assert.equal(p.shell.computer.phase, "asleep", "a refusal is no view");
  p.refusing = null;
  p.clock.tick(60_000);
  p.ui.prewake();
  await settle();
  assert.deepEqual(p.posted(), [WAKE, WAKE]);
  assert.equal(p.shell.computer.phase, "awake");
});
