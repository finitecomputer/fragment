// cell/shell/computer.js's paused shared key's notice (FIN-448):
// `keyPausedSaid`, its words, and `createExpiry`, which draws an idle page
// again as a pause ends, driven by a fake clock; and its Allow, tapped,
// whose answers come in any order.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createAsks } from "../shell/asks.js";
import { createComputer, createExpiry, keyPausedSaid } from "../shell/computer.js";

const paused = (o) => ({ kind: "key_paused", provider: "elevenlabs", spentMicros: 5_030_000, capMicros: 5_000_000, resumesAt: 60_000, ...o });

test("a pause: the key, what it spent this hour, and the computer's one tap; none past its end", () => {
  assert.deepEqual(keyPausedSaid(paused({ allowMicros: 20_000_000 }), 0), { title: "ElevenLabs calls paused: $5.03 this hour", allow: "Allow up to $20 an hour", most: null });
  assert.equal(keyPausedSaid(paused({ allowMicros: 20_000_000 }), 60_000), null);
});

test("above $250 the tap is the most a person may allow; at the most, said so, with no button", () => {
  assert.equal(keyPausedSaid(paused({ capMicros: 320_000_000, allowMicros: 1_000_000_000 }), 0).allow, "Allow up to $1,000 an hour");
  assert.deepEqual(keyPausedSaid(paused({ spentMicros: 1_000_400_000, capMicros: 1_000_000_000 }), 0), {
    title: "ElevenLabs calls paused: $1,000.40 this hour",
    allow: null,
    most: "$1,000 an hour is the most allowed.",
  });
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

// ---- Allow, tapped (#1141's review): the real `createComputer`, its page
// faked as far as the notices' box, the shell's asks (asks.js) as shell.js
// has them. Each call answers only when the test says, so answers come in
// any order: a list read (`load`, as the page's watch asks one) taken in
// only while no view was taken in since it was asked; a tap's answer, a
// read in the same sequence, only while no read was asked after it. ----

class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false, onclick: null });
    this.classList = { add: (c) => (this.className += ` ${c}`) };
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
}
const walk = (n) => [n, ...n.children.flatMap(walk)];
const settle = () => new Promise((r) => setTimeout(r, 0));
const ENDS = Date.now() + 3_600_000;
const pausedKey = (provider, cap) => ({ kind: "key_paused", provider, spentMicros: 5_030_000, capMicros: cap, allowMicros: cap * 4, resumesAt: ENDS });
const viewOf = (...notices) => ({ computer: "computer:aa", phase: "awake", notices });
const COMPUTER = "/api/computers/computer%3Aaa";

function page(t, first) {
  const box = new Node("div", "computer-notices");
  globalThis.document = { hidden: false, addEventListener() {}, getElementById: (id) => (id === "computer-notices" ? box : null) };
  const calls = [];
  const api = (method, path, body) => new Promise((ok, fail) => calls.push({ method, path, body, ok: (v) => ok(v), fail: (e) => fail(e) }));
  const asks = createAsks();
  const reads = { asked: 0, shown: 0, updating: false, listFailed: false };
  const shell = { computer: first };
  let ui = null;
  const computerIs = (v) => {
    if (!v?.computer) return;
    asks.ask("computer");
    shell.computer = v;
    ui.renderComputer();
  };
  // shell.js `load`, as far as the computer: a view taken in while it was
  // out is newer than the one it brings
  const load = async () => {
    const mine = ++reads.asked;
    const taken = asks.now("computer");
    const answer = await api("GET", "/api/computers");
    if (mine < reads.shown) return;
    reads.shown = mine;
    if (asks.current(taken)) shell.computer = answer.computers[0];
    ui.renderComputer();
  };
  const stub = () => new Node("div");
  ui = createComputer({
    api, el: (tag, cls, text) => new Node(tag, cls, text), seg: encodeURIComponent, named: (n) => n, section: stub, line: stub, actionLine: stub,
    computer: () => shell.computer, agents: () => new Map(), computerFailed: () => false, support: () => null, setupPending: () => true,
    made: () => {}, computerIs, openScreen: () => {}, markOffer: () => {}, reads: () => reads, load,
  });
  ui.renderComputer();
  // no pause left behind: its redraw timer let go, whatever the test did
  t.after(() => {
    shell.computer = viewOf();
    ui.renderComputer();
  });
  const rows = () => box.children.filter((r) => r.dataset.kind === "key_paused");
  return {
    calls,
    load,
    // the pauses shown: each key, its button's state, and its lines
    shown: () => rows().map((r) => {
      const button = walk(r).find((n) => n.dataset.action === "allow-key");
      return { key: walk(r).find((n) => n.tag === "strong").textContent, enabled: !!button && !button.disabled, lines: walk(r).filter((n) => n.tag === "p").map((n) => n.textContent) };
    }),
    tap(provider) {
      const row = rows().find((r) => walk(r).some((n) => n.tag === "strong" && n.textContent.toLowerCase().startsWith(provider)));
      walk(row).find((n) => n.dataset.action === "allow-key").onclick();
    },
  };
}
const ELEVEN = "ElevenLabs calls paused: $5.03 this hour";
const XAI = "xAI calls paused: $5.03 this hour";

test("a tap's late answer never takes away a pause a newer read showed: a read answered first", async (t) => {
  const p = page(t, viewOf(pausedKey("elevenlabs", 5_000_000)));
  p.tap("elevenlabs");
  // the CLI sets the key back to its default meanwhile: the page's watch reads the computer
  p.load();
  p.calls[1].ok({ computers: [viewOf(pausedKey("elevenlabs", 5_000_000))] });
  await settle();
  // the tap's answer, the computer as the lift left it, comes after
  p.calls[0].ok(viewOf());
  await settle();
  assert.deepEqual(p.shown(), [{ key: ELEVEN, enabled: true, lines: [] }], "the pause the newer read showed stays, to tap");
});

test("a tap's late answer never takes away a pause a newer read showed: the tap answered first", async (t) => {
  const p = page(t, viewOf(pausedKey("elevenlabs", 5_000_000)));
  p.tap("elevenlabs");
  p.load();
  // the tap's answer first, while the newer read is out
  p.calls[0].ok(viewOf());
  await settle();
  p.calls[1].ok({ computers: [viewOf(pausedKey("elevenlabs", 5_000_000))] });
  await settle();
  assert.deepEqual(p.shown().map((s) => s.key), [ELEVEN], "the newer read is taken in, never let go for the tap's answer");
});

test("two keys tapped, their answers crossing: the older tap's view never puts back a pause the newer one lifted", async (t) => {
  const p = page(t, viewOf(pausedKey("elevenlabs", 5_000_000), pausedKey("xai", 5_000_000)));
  p.tap("elevenlabs");
  p.tap("xai");
  // the computer lifted ElevenLabs, then xAI; xAI's answer comes first
  p.calls[1].ok(viewOf());
  await settle();
  p.calls[0].ok(viewOf(pausedKey("xai", 5_000_000)));
  await settle();
  assert.deepEqual(p.shown(), [], "both lifted: no pause shown");
});

test("the tap refused and the computer's read failing too: the pause stays, its button back with a short line", async (t) => {
  const p = page(t, viewOf(pausedKey("elevenlabs", 5_000_000)));
  p.tap("elevenlabs");
  p.calls[0].fail(Object.assign(new Error("refused"), { status: 503 }));
  await settle();
  assert.equal(p.calls[1].method, "GET", "the computer read instead");
  assert.equal(p.calls[1].path, COMPUTER);
  p.calls[1].fail(new TypeError("Failed to fetch"));
  await settle();
  assert.deepEqual(p.shown(), [{ key: ELEVEN, enabled: true, lines: ["Couldn't confirm that. Try again."] }]);
  // read again meanwhile, the same pause: still unconfirmed
  p.load();
  p.calls[2].ok({ computers: [viewOf(pausedKey("elevenlabs", 5_000_000))] });
  await settle();
  assert.deepEqual(p.shown(), [{ key: ELEVEN, enabled: true, lines: ["Couldn't confirm that. Try again."] }]);
  // tapped again, it goes through
  p.tap("elevenlabs");
  const again = p.calls[3];
  assert.deepEqual([again.method, again.body], ["POST", { from: 5_000_000 }]);
  again.ok(viewOf());
  await settle();
  assert.deepEqual(p.shown(), []);
});

test("the tap's answer lost after the computer lifted the cap, its read failing too: tapped again, the pause goes", async (t) => {
  const p = page(t, viewOf(pausedKey("elevenlabs", 5_000_000)));
  p.tap("elevenlabs");
  p.calls[0].fail(new TypeError("network error"));
  await settle();
  p.calls[1].fail(Object.assign(new Error("deploying"), { status: 503 }));
  await settle();
  assert.deepEqual(p.shown(), [{ key: ELEVEN, enabled: true, lines: ["Couldn't confirm that. Try again."] }], "the last known pause, to tap again");
  // the computer holds $20 now: the page's $5 changes nothing, and its view says no pause
  p.tap("elevenlabs");
  assert.deepEqual(p.calls[2].body, { from: 5_000_000 });
  p.calls[2].ok(viewOf());
  await settle();
  assert.deepEqual(p.shown(), []);
});
