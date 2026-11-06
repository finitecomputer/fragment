// cell/shell/restart-follow.js: a restart the person asked for, followed
// until the computer says it is done, as one explicit state (FIN-395),
// driven with a fake clock and reads and asks held until each test
// answers them (Codex on #1053: it read on hidden and after disposal, said
// "Starting…" over failed reads, and took a failed ask for done; then, at
// fde60d1a, lost a failed ask's decision across a failed read or a pause,
// read while hidden when its ask failed, and let a failure kept on screen
// make a later sleep say Saving).
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { active, createRestartFollow, step, FOLLOW_MS, FOLLOWING, PAUSED, READ_FAILED, FAILED } from "../shell/restart-follow.js";
import { stateSaid } from "../shell/computer.js";

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

// ---- Codex at fde60d1a: a failed ask's decision belongs to the state ----

const REFUSED = "agents are stopped: the credit is used up";

test("its ask refused, the read that would decide it failing, then Check again reading the computer as it was: failed, with the refusal, and Restart offered again", async () => {
  const p = page();
  p.follow.press();
  await p.answerAsk(null, new Error(REFUSED));
  await p.answerRead(null, new Error("offline"));
  assert.equal(p.follow.op().state, READ_FAILED, "the read's failure is said, not the refusal yet");
  assert.equal(p.follow.press(), false, "undecided: no second restart");
  p.follow.readAgain();
  await p.answerRead(at(4, "awake"));
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: FAILED, error: REFUSED });
  assert.equal(p.timers(), 0);
  assert.equal(p.follow.press(), true, "Try again");
});

test("its ask refused, the page hidden and shown while the deciding read is under way: that read dropped, the next decides", async () => {
  const p = page();
  p.follow.press();
  await p.answerAsk(null, new Error(REFUSED));
  p.hidden = true;
  p.follow.visibility();
  await p.answerRead(at(4, "awake"));
  assert.equal(p.follow.op().state, PAUSED, "dropped while hidden");
  p.hidden = false;
  p.follow.visibility();
  await p.answerRead(at(4, "awake"));
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: FAILED, error: REFUSED });
});

test("its ask refused while hidden: nothing read until shown, then one read decides", async () => {
  const p = page();
  p.follow.press();
  p.hidden = true;
  p.follow.visibility();
  await p.answerAsk(null, new Error(REFUSED));
  assert.equal(p.reads.length, 0, "no read while hidden");
  assert.equal(p.timers(), 0);
  assert.equal(p.follow.op().state, PAUSED);
  await p.tick(60_000);
  assert.equal(p.reads.length, 0);
  p.hidden = false;
  p.follow.visibility();
  assert.equal(p.reads.length, 1);
  await p.answerRead(at(4, "awake"));
  assert.equal(p.follow.op().state, FAILED);
});

test("its ask refused after a read failed: nothing read until Check again, then it decides", async () => {
  const p = page();
  p.follow.press();
  await p.tick();
  await p.answerRead(null, new Error("offline"));
  await p.answerAsk(null, new Error(REFUSED));
  assert.equal(p.reads.length, 0, "the page said it waits for Check again");
  assert.deepEqual(p.follow.op(), { asked: 4, at: 0, state: READ_FAILED, error: "offline" });
  p.follow.readAgain();
  await p.answerRead(at(5, "starting"));
  assert.equal(p.follow.op().state, FOLLOWING, "under way after all: followed, the refusal forgotten");
  await p.tick();
  await p.answerRead(at(5, "awake"));
  assert.equal(p.follow.op(), null);
});

test("a restart failed and kept on screen is no active restart: a later sleep of the same start is Going to sleep, one being followed says Saving", async () => {
  const p = page();
  p.follow.press();
  await p.answerAsk(null, new Error(REFUSED));
  await p.answerRead(at(4, "awake"));
  assert.equal(p.follow.op().state, FAILED);
  assert.equal(active(p.follow.op()), false);
  assert.equal(stateSaid(at(4, "sleeping"), active(p.follow.op())), "Going to sleep");
  p.follow.press();
  assert.equal(stateSaid(at(4, "sleeping"), active(p.follow.op())), "Saving");
  assert.equal(active(null), false);
});

// ---- the machine itself ----

test("step is the only judge: unknown events refused, and the same state back when nothing changes", () => {
  const s = step(null, { type: "press", asked: 4, at: 0, hidden: false }).s;
  assert.deepEqual(s, { name: FOLLOWING, asked: 4, at: 0, refused: null, error: null });
  for (const type of ["shown", "check_again", "tick"]) assert.equal(step(s, { type, hidden: false }).s, s, type);
  assert.equal(step(s, { type: "read_ok", v: at(4, "sleeping") }).s, s);
  assert.deepEqual(step(s, { type: "read_ok", v: at(5, "awake") }), { s: null, fx: [{ do: "cancel" }] });
  assert.throws(() => step(s, { type: "nonsense" }));
  const failed = { name: FAILED, asked: 4, at: 0, error: "x" };
  for (const type of ["tick", "read_ok", "read_err", "ask_ok", "ask_err", "hidden", "shown", "check_again"]) {
    assert.equal(step(failed, { type, v: at(4, "awake"), message: "m", hidden: false }).s, failed, `${type} leaves a failure as it is`);
  }
  assert.equal(step(step(null, { type: "press", asked: 4, at: 0, hidden: true }).s, { type: "press", asked: 4, at: 0, hidden: false }).fx.length, 0, "pressed again while it runs: nothing");
});

// Every way its ask can end, every way the read after it can end, the page
// hidden while the ask settles or not, and Check again pressed or not: no
// read while hidden, a timer only while following, Restart offered only
// once failed, and the end the rules give.
test("table: ask outcomes × read outcomes × visibility × Check again", async () => {
  const asks = ["answered", "refused", "lost"];
  const readOuts = ["idle", "underway", "fails"];
  let n = 0;
  for (const ask of asks) {
    for (const readOut of readOuts) {
      for (const hide of [false, true]) {
        for (const again of [false, true]) {
          const name = `${ask} / read ${readOut} / ${hide ? "hidden as it settles" : "shown"} / ${again ? "Check again" : "no Check again"}`;
          const p = page();
          p.follow.press();
          if (hide) {
            p.hidden = true;
            p.follow.visibility();
          }
          const said = ask === "refused" ? REFUSED : "Failed to fetch";
          if (ask === "answered") await p.answerAsk(at(4, "starting"));
          else await p.answerAsk(null, new Error(said));
          if (hide) {
            assert.equal(p.reads.length, 0, `${name}: nothing read while hidden`);
            assert.equal(p.timers(), 0, `${name}: no timer while hidden`);
            p.hidden = false;
            p.follow.visibility();
          } else if (ask === "answered") {
            await p.tick();
          }
          assert.equal(p.reads.length, 1, `${name}: one read`);
          if (readOut === "fails") await p.answerRead(null, new Error("offline"));
          else await p.answerRead(readOut === "idle" ? at(4, "awake") : at(5, "starting"));
          if (readOut === "fails") {
            assert.equal(p.follow.op().state, READ_FAILED, name);
            assert.equal(p.timers(), 0, `${name}: no timer after a failed read`);
            await p.tick(60_000);
            assert.equal(p.reads.length, 0, `${name}: nothing read until Check again`);
            if (again) {
              p.follow.readAgain();
              assert.equal(p.reads.length, 1, `${name}: Check again reads`);
              await p.answerRead(at(4, "awake"));
            }
          }
          const want = readOut === "fails" && !again ? READ_FAILED : readOut === "underway" ? FOLLOWING : ask === "answered" ? FOLLOWING : FAILED;
          const op = p.follow.op();
          assert.equal(op.state, want, name);
          if (want === FAILED) assert.equal(op.error, said, `${name}: its own words`);
          assert.equal(p.timers(), want === FOLLOWING ? 1 : 0, `${name}: a timer only while following`);
          assert.equal(p.follow.press(), want === FAILED, `${name}: Restart offered only once failed`);
          n++;
        }
      }
    }
  }
  assert.equal(n, 36);
});
