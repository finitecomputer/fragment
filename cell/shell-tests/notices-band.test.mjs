// cell/shell/computer.js's notices band (FIN-458): every notice the server
// tells (`GET /api/notices`) drawn the same way, whatever its kind: its
// words, its countdown, and its actions, each a route posted with its body
// and then one more read (the newest wins), or a page opened. The real
// `createComputer`, its page faked as far as the band; each call answers
// only when the test says, so answers come in any order.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createComputer, untilSaid } from "../shell/computer.js";

class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false, onclick: null, attrs: {} });
    this.classList = { add: (c) => (this.className += ` ${c}`) };
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
}
const walk = (n) => [n, ...n.children.flatMap(walk)];
const settle = () => new Promise((r) => setTimeout(r, 0));

// The page's clock and timers, a test's.
function clock(start = 0) {
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

const COMPUTER = "computer:aa";
const RESTART = `/api/computers/${COMPUTER}/restart`;
const allow = (cap) => ({ label: "Allow up to $20 an hour", post: `/api/computers/${COMPUTER}/key-caps/elevenlabs`, body: { from: cap } });
const paused = (o = {}) => ({ id: "key_paused:elevenlabs", kind: "key_paused", level: "warn", title: "ElevenLabs calls paused: $5.03 this hour", actions: [allow(5_000_000)], ...o });

function page(t, first, c = clock(Date.now())) {
  const box = new Node("div", "computer-notices");
  globalThis.document = { hidden: false, addEventListener() {}, removeEventListener() {}, getElementById: (id) => (id === "computer-notices" ? box : null) };
  const calls = [];
  const api = (method, path, body) => new Promise((ok, fail) => calls.push({ method, path, body, ok: (v) => ok(v), fail: (e) => fail(e) }));
  const reads = { asked: 0, shown: 0, updating: false, listFailed: false };
  const shell = { notices: first, computer: { computer: COMPUTER, phase: "awake", generation: 3, notices: [] } };
  let ui = null;
  // shell.js `load`, as far as the notices: an older read's answer that
  // comes after a newer one's is not shown
  const load = async () => {
    const mine = ++reads.asked;
    const answer = await api("GET", "/api/notices");
    if (mine < reads.shown) return;
    reads.shown = mine;
    shell.notices = answer.notices;
    ui.renderComputer();
  };
  const stub = () => new Node("div");
  ui = createComputer({
    api, el: (tag, cls, text) => new Node(tag, cls, text), seg: (s) => s, named: (n) => n, section: stub, line: stub, actionLine: stub,
    computer: () => shell.computer, notices: () => shell.notices, agents: () => new Map(), computerFailed: () => false, setupPending: () => true,
    made: () => {}, computerIs: () => {}, openScreen: () => {}, markOffer: () => {}, reads: () => reads, load, timers: c, now: c.now,
  });
  ui.renderComputer();
  t.after(() => {
    shell.notices = [];
    ui.renderComputer();
    ui.dispose();
  });
  const rows = () => box.children.filter((r) => !r.hidden);
  return {
    box, calls, load, clock: c, shell, ui,
    // each row: its id, its words, and its actions as drawn
    shown: () => rows().map((r) => ({
      id: r.dataset.id,
      words: walk(r).filter((n) => n.tag === "strong" || n.tag === "p").map((n) => n.textContent),
      actions: walk(r).filter((n) => n.tag === "button" || n.tag === "a").map((n) => (n.tag === "a" ? { open: n.href } : { label: n.textContent, enabled: !n.disabled })),
    })),
    tap(id, label) {
      const row = rows().find((r) => r.dataset.id === id);
      walk(row).find((n) => n.tag === "button" && n.textContent === label).onclick();
    },
  };
}

test("a notice of a kind the shell has never seen is drawn like any other: its words, its detail, a button for each post and a link for each page", (t) => {
  const p = page(t, [
    { id: "brand_new", kind: "brand_new", level: "warn", title: "Something new", text: "Said by the server.", detail: "its own words", actions: [{ label: "Do it", post: "/api/new/do", body: { a: 1 } }, { label: "Get help", open: "mailto:help@example.com" }] },
  ]);
  assert.deepEqual(p.shown(), [
    { id: "brand_new", words: ["Something new", "Said by the server.", "its own words"], actions: [{ label: "Do it", enabled: true }, { open: "mailto:help@example.com" }] },
  ]);
  assert.ok(p.box.children[0].className.includes("warn"), "its level, drawn");
  assert.equal(p.box.hidden, false);
});

test("a tap posts its body to its route, then reads again; the newest read decides what shows", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  assert.deepEqual([p.calls[0].method, p.calls[0].path, p.calls[0].body], ["POST", `/api/computers/${COMPUTER}/key-caps/elevenlabs`, { from: 5_000_000 }]);
  assert.equal(p.shown()[0].actions[0].enabled, false, "pressed once");
  // the page's watch reads meanwhile, before the lift: the pause, as it was
  p.load();
  p.calls[1].ok({ notices: [paused()] });
  await settle();
  // the tap answers, and the read after it is the newest
  p.calls[0].ok({ computer: COMPUTER });
  await settle();
  assert.deepEqual([p.calls[2].method, p.calls[2].path], ["GET", "/api/notices"]);
  p.calls[2].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.shown(), [], "lifted: nothing shown");
  assert.equal(p.box.hidden, true);
});

test("a tap refused: the notice stays as read, its button back with a short line, and tapped again it goes through", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[0].fail(Object.assign(new Error("refused"), { status: 503 }));
  await settle();
  assert.equal(p.calls[1].path, "/api/notices", "read again: the read decides");
  p.calls[1].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown(), [{ id: "key_paused:elevenlabs", words: ["ElevenLabs calls paused: $5.03 this hour", "Couldn't do that. Try again."], actions: [{ label: "Allow up to $20 an hour", enabled: true }] }]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[2].ok({ computer: COMPUTER });
  await settle();
  p.calls[3].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour"], "it went through: no line");
});

test("a tap refused whose read shows the notice gone: nothing left behind", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[0].fail(new TypeError("Failed to fetch"));
  await settle();
  p.calls[1].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.shown(), []);
  // a new pause of the same key later is a new notice, with no old line
  p.load();
  p.calls[2].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour"]);
});

test("a countdown: said live, each minute, and the notice gone as it ends, with one timer at a time and no call", (t) => {
  const c = clock(0);
  const p = page(t, [paused({ until: { at: 150_000, said: "Calls resume" } }), { id: "update:abc", kind: "update", level: "info", title: "An update is ready", actions: [] }], c);
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour", "Calls resume in 3 min."]);
  assert.deepEqual(c.pending(), [30_000], "one timer, as its minute changes");
  const button = p.box.children[0].children[1].children[0];
  c.tick(30_000);
  assert.deepEqual(p.shown()[0].words.at(-1), "Calls resume in 2 min.");
  assert.equal(p.box.children[0].children[1].children[0], button, "said again in place: its button stays");
  c.tick(59_999);
  assert.deepEqual(p.shown()[0].words.at(-1), "Calls resume in 2 min.");
  c.tick(1);
  assert.deepEqual(p.shown()[0].words.at(-1), "Calls resume in under a minute.");
  c.tick(59_999);
  assert.equal(p.shown().length, 2);
  c.tick(1);
  assert.deepEqual(p.shown().map((r) => r.id), ["update:abc"], "ended: gone, the rest as they were");
  assert.deepEqual(c.pending(), [], "nothing left to wait for");
  assert.equal(p.calls.length, 0, "no call");
});

test("a countdown in words", () => {
  const u = { at: 0, said: "Calls resume" };
  assert.equal(untilSaid(u, -59_000), "Calls resume in under a minute.");
  assert.equal(untilSaid(u, -60_000), "Calls resume in under a minute.");
  assert.equal(untilSaid(u, -61_000), "Calls resume in 2 min.");
  assert.equal(untilSaid(u, -3_600_000), "Calls resume in 60 min.");
  assert.equal(untilSaid(u, -3_660_000), "Calls resume in 1 h 1 min.");
  assert.equal(untilSaid(u, -7_200_000), "Calls resume in 2 h.");
});

test("the computer's Restart is its restart, followed where the notice was, and the notices it answers wait while it runs", async (t) => {
  const update = { id: "update:abc", kind: "update", level: "info", title: "An update is ready", actions: [{ label: "Restart", post: RESTART, body: { generation: 3 } }, { label: "Later", post: `/api/computers/${COMPUTER}/notices/later`, body: { version: "abc" } }] };
  const disk = { id: "disk_filling", kind: "disk_filling", level: "warn", title: "Your computer is almost full (3.2 GB left)", actions: [] };
  const p = page(t, [update, disk]);
  p.tap("update:abc", "Restart");
  assert.deepEqual([p.calls[0].method, p.calls[0].path, p.calls[0].body], ["POST", RESTART, { generation: 3 }], "the restart's own ask, of the start shown");
  assert.deepEqual(p.shown().map((r) => r.id ?? r.words[0]), ["Restarting your computer", "disk_filling"], "its progress where the update was; the disk's notice stays");
  assert.deepEqual(p.shown()[0].words, ["Restarting your computer"]);
});
