// An agent's screen backs off to silence when no one is at it (Paul,
// 2026-11-18: "just like computers go to sleep if you don't move your
// mouse!"): images/hermes/screen/attend.js, the page's own clock of its
// person, and cell/shell/screen-active.js, the shell's word that its person
// is at it. An open port tab holds its computer only while its page speaks
// (docs/computers.md, Ports), so a screen that stops speaking lets its
// computer sleep, and one whose sockets went opens them again only on its
// person's press: never by itself, never in a loop. Each against a fake
// clock and timers, a day in milliseconds.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { attention, PAUSE_MS } from "../../images/hermes/screen/attend.js";
import { screenActive, ACTIVE_EVERY_MS } from "../shell/screen-active.js";

const MIN = 60_000;
const DAY = 24 * 60 * MIN;

// A screen's attention on a fake clock: its timers fire as `to` passes
// them, and what it asked of the page is counted.
function screenAt() {
  let t = 1_800_000_000_000;
  let next = 1;
  const timers = new Map();
  const did = { pause: 0, resume: 0, wake: 0 };
  const a = attention({
    now: () => t,
    setTimer: (f, ms) => {
      const id = next++;
      timers.set(id, { at: t + ms, f });
      return id;
    },
    clearTimer: (id) => timers.delete(id),
    pause: () => did.pause++,
    resume: () => did.resume++,
    wake: () => did.wake++,
  });
  const to = (at) => {
    for (;;) {
      const due = [...timers.entries()].filter(([, x]) => x.at <= at).sort((x, y) => x[1].at - y[1].at)[0];
      if (!due) break;
      timers.delete(due[0]);
      t = due[1].at;
      due[1].f();
    }
    t = at;
  };
  return { a, did, timers, now: () => t, by: (ms) => to(t + ms) };
}

test("a screen no one touches pauses after PAUSE_MS, once; a touch resumes it", () => {
  const s = screenAt();
  s.by(PAUSE_MS - 1);
  assert.equal(s.a.state(), "live", "streaming until its pause");
  // invalid: no use of a kind it does not know
  assert.throws(() => s.a.used("hover"));
  s.by(1);
  assert.deepEqual([s.a.state(), s.did.pause], ["paused", 1]);
  s.by(DAY);
  assert.equal(s.did.pause, 1, "paused once, and nothing more, a day on");
  assert.equal(s.timers.size, 0, "no timer runs while paused");
  s.a.used("move");
  assert.deepEqual([s.a.state(), s.did.resume, s.did.wake], ["live", 1, 0], "the pointer moving resumes it: its computer is up");
  s.by(PAUSE_MS);
  assert.deepEqual([s.a.state(), s.did.pause], ["paused", 2], "and it pauses again with no one at it");
});

test("a person at it keeps it streaming for as long as they are: a touch a minute for a day", () => {
  const s = screenAt();
  for (let m = 0; m < 24 * 60; m++) {
    s.by(MIN);
    s.a.used(m % 2 ? "move" : "press");
  }
  assert.deepEqual([s.a.state(), s.did.pause], ["live", 0]);
  assert.ok(s.timers.size <= 1, "one timer at a time");
  s.by(PAUSE_MS);
  assert.deepEqual([s.a.state(), s.did.pause], ["paused", 1], "PAUSE_MS after the last touch");
});

test("a screen whose sockets went opens them again only on its person's press, once, never by itself", () => {
  const s = screenAt();
  s.by(PAUSE_MS);
  // its computer slept under it: its sockets closed
  s.a.gone();
  s.by(7 * DAY);
  assert.deepEqual([s.a.state(), s.did.wake, s.did.resume], ["gone", 0, 0], "a week with no one: nothing opened");
  assert.equal(s.a.attended(), false, "and no stream of its own comes back");
  s.a.used("move");
  assert.deepEqual([s.a.state(), s.did.wake], ["gone", 0], "a hand passing over it wakes nothing");
  s.a.used("press");
  assert.deepEqual([s.a.state(), s.did.wake], ["waking", 1], "a press opens them, once");
  s.a.used("move");
  assert.deepEqual([s.did.wake, s.did.resume], [1, 0], "a hand passing while they open opens nothing");
  assert.equal(s.a.attended(), true, "a stream slow to come may try again, for its person");
  // its stream never came, its tries ran out: stuck no longer than the next press
  s.by(PAUSE_MS);
  assert.deepEqual([s.a.state(), s.a.attended()], ["waking", false], "no try by itself past PAUSE_MS");
  s.a.used("press");
  assert.deepEqual([s.a.state(), s.did.wake, s.did.resume, s.a.attended()], ["waking", 1, 1, true], "a press tries its stream again (the page opens only what is not open), never its sockets twice");
  // replay: they did not come up (still starting, say): the next press tries again
  s.a.gone();
  s.by(DAY);
  assert.deepEqual([s.a.state(), s.did.wake], ["gone", 1], "a failed wake tries nothing by itself");
  s.a.used("press");
  s.a.up();
  assert.deepEqual([s.a.state(), s.did.wake], ["live", 2]);
  s.a.up();
  assert.equal(s.a.state(), "live", "an up heard twice is the same");
  s.by(PAUSE_MS - 1);
  assert.equal(s.a.state(), "live", "its pause counts from that press");
  s.by(1);
  assert.equal(s.a.state(), "paused");
});

test("a stream that comes back by itself is no use: a desktop restarting keeps nothing awake", () => {
  const s = screenAt();
  for (let i = 0; i < 100; i++) {
    s.by(10_000);
    s.a.up();
  }
  assert.deepEqual([s.a.state(), s.did.pause], ["paused", 1], "paused PAUSE_MS after its last touch, whatever came back");
  assert.equal(s.a.attended(), false, "and none of it opens again");
  // waking, attended only within PAUSE_MS of its press
  s.a.gone();
  s.a.used("press");
  s.by(PAUSE_MS - 1);
  assert.equal(s.a.attended(), true);
  s.by(1);
  assert.equal(s.a.attended(), false, "a stream that never came stops trying");
});

test("the shell tells the screens on view that its person is at it, ACTIVE_EVERY_MS apart, well within a pause", () => {
  assert.ok(ACTIVE_EVERY_MS * 4 < PAUSE_MS, "never a pause while someone is at the shell");
  let t = 0;
  let on = [];
  const told = [];
  const tell = screenActive({ now: () => t, frames: () => on, tell: (f) => (f === "broken" ? (() => { throw new Error("gone"); })() : told.push(f)) });
  assert.equal(tell(), 0, "no screen on view: nothing told");
  on = ["a"];
  assert.equal(tell(), 1, "a screen shown: told at the next use, not a window later");
  t += ACTIVE_EVERY_MS - 1;
  assert.equal(tell(), 0, "once a window");
  t += 1;
  on = ["broken", "a", "b"];
  assert.equal(tell(), 2, "a frame that throws stops none of the others");
  assert.deepEqual(told, ["a", "a", "b"]);
});
